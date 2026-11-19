//! Native AV1 + H.264 decoder adapted to the OFFICIAL VideoSink trait.
//! Codec bytes are the official Punktfunk QUIC access units; there is no
//! Moonlight/GameStream protocol code in this backend.
use std::ptr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{anyhow, bail, ensure, Result};
use pf_nx_app::core::caps::VideoCaps;
use pf_nx_app::core::media::{FeedFlags, MediaClock, NotReady, VideoSink, VideoSinkCaps};
use pf_nx_app::platform::{StreamSpec, VideoDecoders};
use punktfunk_core::quic;

use crate::display::{VideoPicture, VideoSlot, bounded_video_handoff, pacing_override_name};
use crate::ffi;
use crate::nv12_pool::Nv12Pool;

mod source_picture;

pub struct NativeVideo { pub(crate) frames: Arc<VideoSlot> }

const MAX_WIDTH: u32 = 3840;
const MAX_HEIGHT: u32 = 2160;
// Upper bound for Punktfunk access units. The previous working AV1 8-bit
// VideoDecoder used this same SDK hint with block-linear surfaces.
const MAX_AU: usize = 12 * 1024 * 1024;

fn sdk_max_sample_bytes(height: u32, bit_depth: u8) -> u64 {
    // The later 1 MiB experiment did not fix SDK 0xfa66f. Restore the
    // measured-working 8-bit resource hint together with BlockLinear.
    // Keep the separate Main10 experimental limits from Moonlight NX2.
    if bit_depth == 8 { MAX_AU as u64 }
    else if height > 1080 { 4 * 1024 * 1024 }
    else { 1024 * 1024 }
}
const MAX_DRAIN: usize = 4;
const CODEC_H264_NATIVE: u32 = 0;
const CODEC_AV1_NATIVE: u32 = 3;
// A fresh libounce AV1 decoder must receive a sequence header before
// a frame OBU. An incoming stream may start on a delta frame (e.g. TD+FRAME).
// Validate low-overhead OBU lengths without allocating or scanning payloads.
fn av1_has_sequence_header(au: &[u8]) -> bool {
    let mut at = 0usize;
    while at < au.len() {
        let header = au[at];
        // Forbidden/reserved bits and missing size field are invalid.
        if (header & 0x81) != 0 || (header & 0x02) == 0 { return false; }
        let kind = (header >> 3) & 0x0f;
        let extension = usize::from((header & 0x04) != 0);
        let Some(mut pos) = at.checked_add(1 + extension) else { return false };
        if pos >= au.len() { return false; }
        let mut len = 0usize;
        let mut complete = false;
        for shift in (0..56).step_by(7) {
            let Some(&byte) = au.get(pos) else { return false };
            pos += 1;
            len |= usize::from(byte & 0x7f) << shift;
            if (byte & 0x80) == 0 { complete = true; break; }
        }
        if !complete { return false; }
        let Some(end) = pos.checked_add(len) else { return false };
        if end > au.len() { return false; }
        if kind == 1 { return true; }
        at = end;
    }
    false
}

// The libounce VideoDecoder configuration is a resource/throughput hint,
// NOT a cap on the Punktfunk encoded stream. 1440p120 AV1 has been observed
// with a decoder session configured at 60, so keep the two rates separate.
const VIDEO_MAX_FPS: u32 = 60;
const STREAM_MAX_FPS: u32 = 120;
const VIDEO_PIXEL_RATE: u64 = 248_832_000;

fn decoder_fps_limit(width: u32, height: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 { return VIDEO_MAX_FPS; }
    (VIDEO_PIXEL_RATE / pixels).min(u64::from(VIDEO_MAX_FPS)) as u32
}

fn negotiated_fps_limit(width: u32, height: u32) -> u32 {
    // Stream FPS and the hardware decoder's nominal resource hint are
    // independent. 1080p/1440p at 120 fps is verified on the console.
    // A docked 4K TV at 60 Hz must be requested as 4K60, not 4K30 solely
    // because VideoDecoderConfig uses a 30-fps allocation hint at 4K.
    // 4K60 throughput is still experimental; failures remain visible.
    if width <= 2560 && height <= 1440 { STREAM_MAX_FPS }
    else if width <= MAX_WIDTH && height <= MAX_HEIGHT { VIDEO_MAX_FPS }
    else { decoder_fps_limit(width, height) }
}

impl VideoDecoders for NativeVideo {
    fn caps(&self) -> VideoCaps {
        // Main10 input is experimental; opt in at COMPILE time with
        // PF2_ENABLE_MAIN10=1. Native HDR scanout remains disabled: PQ is
        // tone-mapped to SDR. The default release advertises only 8-bit.
        let main10 = matches!(option_env!("PF2_ENABLE_MAIN10"), Some("1"));
        VideoCaps {
            h264: true, hevc: false, av1: true, pyrowave: false,
            // Opt-in Main10/PQ input, presented with an SDR tone map until the
            // OunceVK/VI2 HDR10 swapchain can be verified on real hardware.
            ten_bit: main10, hdr: main10, max_channels: 2,
        }
    }

    fn max_stream_fps(&self, width: u32, height: u32) -> u32 {
        negotiated_fps_limit(width, height)
    }

    fn open(&self, spec: &StreamSpec) -> Result<Box<dyn VideoSink>> {
        ensure!(spec.width > 0 && spec.height > 0 && spec.width <= MAX_WIDTH &&
                spec.height <= MAX_HEIGHT && spec.width % 2 == 0 && spec.height % 2 == 0,
                "unsupported hardware decode resolution {}x{}", spec.width, spec.height);
        ensure!((spec.bit_depth == 8 || (spec.bit_depth == 10 && spec.codec == quic::CODEC_AV1))
                && !spec.chroma444,
                "Switch 2 supports NV12 8-bit and experimental AV1 Main10 P010 4:2:0");
        ensure!(spec.bit_depth != 10 || matches!(option_env!("PF2_ENABLE_MAIN10"), Some("1")),
            "AV1 Main10 is experimental: rebuild with PF2_ENABLE_MAIN10=1 to enable SDR tone mapping");
        if spec.bit_depth == 10 {
            tracing::warn!(
                "Switch 2 AV1 Main10: P010 decoder path is experimental; HDR10 PQ is tone-mapped to SDR RGBA8 (native HDR scanout not yet available)"
            );
        }
        let max_fps = negotiated_fps_limit(spec.width, spec.height);
        ensure!(spec.refresh_hz > 0 && spec.refresh_hz <= max_fps,
                "unsupported stream rate {}x{}@{} (max {})",
                spec.width, spec.height, spec.refresh_hz, max_fps);
        tracing::info!("switch2 video rate: incoming={} fps decoder_config={} fps {}x{}",
            spec.refresh_hz, decoder_fps_limit(spec.width, spec.height), spec.width, spec.height);
        let codec = match spec.codec {
            quic::CODEC_H264 => CODEC_H264_NATIVE,
            quic::CODEC_AV1 => CODEC_AV1_NATIVE,
            _ => bail!("Switch 2 decoder cannot decode negotiated codec {}", spec.codec),
        };
        // AV1 has no Annex B SPS/PPS. H.264 opens after its first access unit
        // provides parameter sets, before submitting that exact same AU.
        // The host's existing WGC monitor may encode at a size DIFFERENT
        // from the requested session mode. Match the desktop client: initialize
        // AV1 8-bit from its first sequence header, not from client.mode().
        // Main10 is experimental and deliberately unchanged.
        let decoder = if codec == CODEC_AV1_NATIVE && spec.bit_depth == 10 {
            Some(DecoderSession::new(spec, codec, &[])? )
        } else { None };
        // A new session may change from smoothing to newest-only. Clear old
        // queued pictures BEFORE altering capacity: the single-slot queue's
        // debug invariant requires older=None when bounded=false.
        self.frames.frame.lock().map_err(|_| anyhow!("poisoned frame slot"))?.clear();
        let bounded = bounded_video_handoff(spec.smoothing);
        self.frames.smoothing_enabled.store(bounded, Ordering::Relaxed);
        tracing::info!("switch2 frame pacing session: bounded={} override={} requested_smoothing={} capacity={} refresh_hz={}",
            bounded, pacing_override_name(), spec.smoothing, if bounded { 2 } else { 1 }, spec.refresh_hz);
        Ok(Box::new(NativeSink {
            spec: spec.clone(), codec,
            nv12_pool: Arc::new(Nv12Pool::default()),
            shared: self.frames.clone(),
            inner: Mutex::new(decoder), started: Instant::now(),
            av1_sequence_seen: AtomicBool::new(false),
            h264_bootstrap: Mutex::new(source_picture::H264Bootstrap::default()),
            last_timing_log_ms: AtomicU64::new(0),
            encoded_units: AtomicU64::new(0), decoded_frames: AtomicU64::new(0),
            last_encoded_report: AtomicU64::new(0), last_decoded_report: AtomicU64::new(0),
        }))
    }

    fn lane_name(&self, codec: u8) -> &'static str {
        if codec == quic::CODEC_AV1 { "libounce AV1 (NV12)" }
        else if codec == quic::CODEC_H264 { "libounce H.264 (NV12)" }
        else { "unsupported" }
    }
}

struct DecoderSession { handle: *mut ffi::Handle, width: u32, height: u32 }
// The pointer is never accessed concurrently; the C SDK says decoder calls
// are serialized and NativeSink::inner is a Mutex.
unsafe impl Send for DecoderSession {}

impl DecoderSession {
    fn new(spec: &StreamSpec, codec: u32, config: &[u8]) -> Result<Self> {
        let mut handle = ptr::null_mut();
        let args = ffi::DecoderSettings {
            // Hardware decoder resource hint (60 or pixel-rate-limited),
            // intentionally independent from negotiated encoded FPS.
            width: spec.width, height: spec.height, fps: decoder_fps_limit(spec.width, spec.height),
            codec, image_count: 4, bit_depth: spec.bit_depth as u32,
            codec_data: if config.is_empty() { ptr::null() } else { config.as_ptr() },
            codec_data_length: config.len(), max_sample_bytes: sdk_max_sample_bytes(spec.height, spec.bit_depth as u8),
        };
        tracing::info!(
            "switch2 decoder SDK: max_sample_size={} bytes at {}x{}, {}-bit {}",
            args.max_sample_bytes, spec.width, spec.height, spec.bit_depth,
            if spec.bit_depth == 8 {
                "8-bit block-linear CopyFrame"
            } else { "pitch-linear Main10 (experimental)" }
        );
        // SAFETY: config lives for the duration of the synchronous open call.
        ffi::check("videoDecoderCreate", unsafe { ffi::pf2_decoder_open(&mut handle, &args) })?;
        ensure!(!handle.is_null(), "decoder returned no handle");
        Ok(Self { handle, width: spec.width, height: spec.height })
    }
}

impl Drop for DecoderSession {
    fn drop(&mut self) {
        // SAFETY: unique owned decoder, no GPU/decoder pointers escape the feed.
        unsafe { ffi::pf2_decoder_close(self.handle) };
    }
}

struct NativeSink {
    spec: StreamSpec,
    codec: u32,
    nv12_pool: Arc<Nv12Pool>,
    shared: Arc<VideoSlot>,
    inner: Mutex<Option<DecoderSession>>,
    started: Instant,
    av1_sequence_seen: AtomicBool,
    h264_bootstrap: Mutex<source_picture::H264Bootstrap>,
    last_timing_log_ms: AtomicU64,
    encoded_units: AtomicU64,
    decoded_frames: AtomicU64,
    last_encoded_report: AtomicU64,
    last_decoded_report: AtomicU64,
}

impl MediaClock for NativeSink {
    fn now_ns(&self) -> u64 { self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64 }
}

impl VideoSink for NativeSink {
    fn name(&self) -> &'static str { "libounce native video" }
    fn caps(&self) -> VideoSinkCaps {
        VideoSinkCaps { pts: true, partial_au: false, flush: true, intra_only: false }
    }
    fn clock(&self) -> Option<&dyn MediaClock> { Some(self) }

    fn feed(&self, au: &[u8], pts_ns: u64, flags: FeedFlags) -> Result<()> {
        ensure!(au.len() <= MAX_AU, "access unit too large");
        if au.is_empty() || !flags.complete { return Err(NotReady.into()); }
        let feed_started = Instant::now();
        self.encoded_units.fetch_add(1, Ordering::Relaxed);
        // If we attached in the middle of a GOP, request a new sequence-
        // header/keyframe anchor via the existing NotReady -> NeedKeyframe path.
        // Never submit a delta frame to a freshly opened AV1 decoder.
        if self.codec == CODEC_AV1_NATIVE
            && !self.av1_sequence_seen.load(Ordering::Relaxed)
            && !av1_has_sequence_header(au)
        {
            return Err(NotReady.into());
        }
        let mut locked = self.inner.lock().map_err(|_| anyhow!("decoder mutex poisoned"))?;
        if self.codec == CODEC_AV1_NATIVE && self.spec.bit_depth == 8 {
            // Read geometry from the same AV1 sequence header desktop clients
            // parse. The host's StreamConfig may not name the WGC source size.
            if let Some((width, height)) = source_picture::av1_dimensions(au) {
                let changed = locked.as_ref().is_none_or(|d|
                    d.width != width || d.height != height);
                if changed {
                    let mut decoded_spec = self.spec.clone();
                    decoded_spec.width = width;
                    decoded_spec.height = height;
                    // Release the old SDK allocation before opening the new size.
                    *locked = None;
                    *locked = Some(DecoderSession::new(&decoded_spec, self.codec, &[])?);
                    tracing::info!(
                        width, height,
                        requested_width = self.spec.width,
                        requested_height = self.spec.height,
                        "switch2 AV1 source geometry: decoder sized from sequence header; renderer unchanged"
                    );
                }
            } else if locked.is_none() {
                // The first AU can be an inter frame. Ask for a new keyframe;
                // never poison the fresh SDK decoder with a missing header.
                return Err(NotReady.into());
            }
        } else if self.codec == CODEC_H264_NATIVE {
            // Unlike AV1, libounce H.264 requires Annex-B SPS+PPS at Create.
            // They may arrive in separate AUs; retain a bounded copy and do
            // not start/restart until an IDR can anchor reference pictures.
            let mut headers = self.h264_bootstrap.lock()
                .map_err(|_| anyhow!("H.264 bootstrap mutex poisoned"))?;
            match headers.observe(au, locked.is_some()) {
                source_picture::H264Decision::Continue => {},
                source_picture::H264Decision::NeedKeyframe => return Err(NotReady.into()),
                source_picture::H264Decision::Reopen { config, dimensions } => {
                    let (width, height) = dimensions.unwrap_or((self.spec.width, self.spec.height));
                    ensure!(width > 0 && height > 0 && width <= MAX_WIDTH && height <= MAX_HEIGHT
                        && width % 2 == 0 && height % 2 == 0,
                        "H.264 SPS requests unsupported geometry {}x{}", width, height);
                    let mut decoded_spec = self.spec.clone();
                    decoded_spec.width = width;
                    decoded_spec.height = height;
                    // Drop the old decoder BEFORE allocating the replacement.
                    *locked = None;
                    *locked = Some(DecoderSession::new(&decoded_spec, self.codec, &config)?);
                    headers.activated(config);
                    tracing::info!(width, height,
                        "switch2 H264 source geometry: decoder anchored on IDR with cached SPS/PPS; renderer unchanged");
                }
            }
        }
        let decoder = locked.as_ref().ok_or_else(|| anyhow!("missing active decoder"))?;
        // This is a bounded wait for output buffers; no unbounded deadlock on disconnect.
        // SAFETY: access unit lives through the call and decoder held exclusively by mutex.
        let rc = unsafe { ffi::pf2_decoder_submit(
            decoder.handle, au.as_ptr(), au.len(), pts_ns.min(i64::MAX as u64) as i64,
            4_000_000) };
        if rc == ffi::NOT_READY { return Err(NotReady.into()); }
        if rc != ffi::OK {
            let sdk = unsafe { ffi::pf2_decoder_last_sdk_result(decoder.handle) };
            // SAFETY: serial decoder access; flush permits a subsequent re-anchor.
            let _ = unsafe { ffi::pf2_decoder_flush(decoder.handle) };
            bail!("decoder submit failed: bridge={rc}, sdk={sdk:#x}");
        }
        if self.codec == CODEC_AV1_NATIVE
            && !self.av1_sequence_seen.swap(true, Ordering::Relaxed)
        {
            tracing::info!("switch2 AV1 sequence header submitted; waiting for first decoded picture");
        }
        let submit_us = feed_started.elapsed().as_micros();
        let width = decoder.width as usize;
        let mut max_acquire_us = 0u128;
        let mut max_copy_us = 0u32;
        let mut pack_us = 0u128;
        let mut decoded = 0u32;
        let height = decoder.height as usize;
        // The C decoder uses persistent NV12 staging buffers (d->y, d->uv).
        // Each acquire overwrites them, but release does not free them. Drain
        // completed pictures and convert only the newest. Older pictures would
        // be overwritten in VideoSlot immediately, wasting a full RGBA convert
        // each (up to MAX_DRAIN conversions per incoming AU previously).
        // Reuse the caller-owned packed NV12 allocation for both 8-bit codecs.
        // AV1 and H.264 8-bit: SDK CopyFrame untiles block-linear surfaces
        // directly into the caller-owned NV12 allocation.
        // With PF2_DIRECT_NV12_OUTPUT unset, both codecs retain their original
        // native staging + Rust packing path. P010 is always excluded.
        // Deliberately use a local `let`: matching string literals in a
        // compile-time `const` requires unstable const string comparison.
        // `option_env!` is still resolved at compile time and optimized away.
        let direct_pack = matches!(option_env!("PF2_DIRECT_NV12_OUTPUT"), Some("1"))
            && self.spec.bit_depth == 8
            && matches!(self.spec.codec, quic::CODEC_AV1 | quic::CODEC_H264);
        let direct_bytes = width.checked_mul(height)
            .and_then(|p| p.checked_mul(3)).map(|n| n / 2)
            .ok_or_else(|| anyhow!("direct NV12 output dimensions overflow"))?;
        let mut direct_nv12 = if direct_pack {
            Some(self.nv12_pool.take(direct_bytes))
        } else { None };
        let mut newest: Option<ffi::Frame> = None;
        for _ in 0..MAX_DRAIN {
            let mut f = ffi::Frame::default();
            let acquire_started = Instant::now();
            let rc = if let Some(buffer) = direct_nv12.as_mut() {
                // SAFETY: capacity is at least direct_bytes, the pointer does
                // not move until the native SDK copy returns, and all decoder
                // calls remain serialized on self.inner's mutex.
                unsafe { ffi::pf2_decoder_acquire_into(decoder.handle, &mut f,
                    buffer.as_mut_ptr(), buffer.capacity(), 0) }
            } else {
                unsafe { ffi::pf2_decoder_acquire(decoder.handle, &mut f, 0) }
            };
            let acquire_us = acquire_started.elapsed().as_micros();
            if rc == ffi::NOT_READY { break; }
            if rc != ffi::OK {
                let sdk = unsafe { ffi::pf2_decoder_last_sdk_result(decoder.handle) };
                let stage = unsafe { ffi::pf2_decoder_last_sdk_stage(decoder.handle) };
                // Record the small compressed AU prefix only on decoder failure:
                // it lets us distinguish an AV1 bitstream issue from SDK setup.
                bail!("video decoder acquire: bridge={rc}, sdk={sdk:#x}, stage={stage}, au_bytes={}, au_prefix={:?} (2=get-frame, 3=copy-frame, 5=plane layout)",
                      au.len(), &au[..au.len().min(12)]);
            }
            max_acquire_us = max_acquire_us.max(acquire_us);
            max_copy_us = max_copy_us.max(unsafe { ffi::pf2_decoder_last_copy_us(decoder.handle) });
            // SAFETY: in the old path plane pointers address native staging;
            // in direct mode they alias the Rust Vec, which stays allocated
            // until after the SDK image is released. Both remain valid here.
            let release_rc = unsafe { ffi::pf2_decoder_release(decoder.handle) };
            ffi::check("decoder release image", release_rc)?;
            newest = Some(f);
            decoded += 1;
        }
        if let Some(f) = newest {
            ensure!(f.width > 0 && f.height > 0 && f.width as usize <= width &&
                    f.height as usize <= height && f.width % 2 == 0 && f.height % 2 == 0,
                    "decoded picture geometry invalid");
            // Packed NV12 (8-bit) or P010 (10 bits in 16-bit little-endian).
            // Ganesh uploads the packed bytes as Gray8 and reconstructs samples
            // on the GPU. No per-pixel CPU conversion in the normal path.
            // The decoder owns its staging planes; copy before the next acquire.
            let row_bytes = (f.width as usize) * if self.spec.bit_depth == 10 { 2 } else { 1 };
            ensure!(f.pitch_y as usize >= row_bytes && f.pitch_uv as usize >= row_bytes &&
                    !f.plane_y.is_null() && !f.plane_uv.is_null(),
                    "invalid decoded NV12 planes");
            let frame_h = f.height as usize;
            let pack_started = Instant::now();
            let y_bytes = row_bytes * frame_h;
            let uv_bytes = y_bytes / 2;
            let mut nv12 = if let Some(buffer) = direct_nv12.take() {
                // The bridge validates dimensions and stride after SDK
                // CopyFrame for AV1/H.264 8-bit.
                ensure!(f.width as usize == width && f.height as usize == height &&
                        f.pitch_y as usize == row_bytes && f.pitch_uv as usize == row_bytes &&
                        f.plane_y == buffer.as_ptr() &&
                        f.plane_uv == unsafe { buffer.as_ptr().add(y_bytes) },
                        "direct NV12 bridge returned unexpected planes");
                buffer
            } else {
                self.nv12_pool.take(y_bytes + uv_bytes)
            };
            if direct_pack {
                // The native SDK already filled every packed byte; no CPU
                // memcpy is necessary. Length is set only after validation.
                unsafe { nv12.set_len(y_bytes + uv_bytes) };
            } else {
            // SAFETY: the C bridge guarantees two persistent staging planes
            // with row strides >= width for the returned dimensions. Every
            // byte of the destination is initialized below, before set_len.
            // Avoid zero-filling ~3 MiB/frame only to overwrite it at 1080p120.
            unsafe {
                let dst = nv12.as_mut_ptr();
                if f.pitch_y as usize == row_bytes {
                    ptr::copy_nonoverlapping(f.plane_y, dst, y_bytes);
                } else {
                    for row in 0..frame_h {
                        ptr::copy_nonoverlapping(
                            f.plane_y.add(row * f.pitch_y as usize),
                            dst.add(row * row_bytes), row_bytes);
                    }
                }
                if f.pitch_uv as usize == row_bytes {
                    ptr::copy_nonoverlapping(f.plane_uv, dst.add(y_bytes), uv_bytes);
                } else {
                    for row in 0..frame_h / 2 {
                        ptr::copy_nonoverlapping(
                            f.plane_uv.add(row * f.pitch_uv as usize),
                            dst.add(y_bytes + row * row_bytes), row_bytes);
                    }
                }
                nv12.set_len(y_bytes + uv_bytes);
            }
            pack_us = pack_started.elapsed().as_micros();
            } // Legacy native staging -> Rust pool packing only.
            if direct_pack { pack_us = 0; }
            let mut picture = VideoPicture {
                width: f.width, height: f.height, rgba: Vec::new(), nv12,
                nv12_pool: Arc::clone(&self.nv12_pool),
                bit_depth: self.spec.bit_depth as u8,
                pq: self.spec.color.transfer == quic::ColorInfo::TRC_PQ,
                matrix: if self.spec.bit_depth == 10 {
                    self.spec.color.matrix as i32
                } else if self.spec.color.matrix == 1 { 1 } else { 0 },
                full_range: self.spec.color.full_range as i32,
                capture_pts_ns: flags.host_pts_ns,
                decoded_at: Instant::now(), published_at: Instant::now(),
                video_e2e: self.spec.video_e2e.clone(),
            };
            // Publish two completed pictures at most, never hardware decoder
            // handles. An opt-out build retains the old newest-wins semantics.
            // Release overwritten large NV12 allocations outside the mutex.
            let superseded = {
                let mut slot = self.shared.frame.lock()
                    .map_err(|_| anyhow!("poisoned frame slot"))?;
                picture.published_at = Instant::now();
                slot.push(picture, self.shared.smoothing_enabled.load(Ordering::Relaxed))
            };
            if superseded.is_some() {
                self.shared.replaced.fetch_add(1, Ordering::Relaxed);
            }
            drop(superseded);
        }
        if let Some(unused) = direct_nv12.take() {
            // No decoded picture: return the still-owned allocation to the
            // pool instead of allocating again on the next access unit.
            self.nv12_pool.give_back(unused);
        }
        if decoded > 0 {
            // The SDK can deliver several pictures from one AU. Only the last
            // is copied to VideoSlot; the earlier ones were never displayed.
            if decoded > 1 {
                self.shared.replaced.fetch_add(decoded - 1, Ordering::Relaxed);
            }
            self.decoded_frames.fetch_add(u64::from(decoded), Ordering::Relaxed);
            let elapsed_ms = self.started.elapsed().as_millis() as u64;
            let previous = self.last_timing_log_ms.load(Ordering::Relaxed);
            if elapsed_ms.saturating_sub(previous) >= 1000 &&
                self.last_timing_log_ms.compare_exchange(previous, elapsed_ms,
                    Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                let incoming = self.encoded_units.load(Ordering::Relaxed);
                let output = self.decoded_frames.load(Ordering::Relaxed);
                let incoming_sec = incoming.saturating_sub(self.last_encoded_report.swap(incoming, Ordering::Relaxed));
                let decoded_sec = output.saturating_sub(self.last_decoded_report.swap(output, Ordering::Relaxed));
                let (pool_reuses, pool_allocations) = self.nv12_pool.counts();
                tracing::info!(
                    "switch2 video pipeline: codec={} incoming_au_s={} decoded_s={} feed_us={} submit_us={} acquire_us={} native_nv12_copy_us={} nv12_pack_us={} direct_nv12={} nv12_pool_reuses={} nv12_pool_allocations={} cpu_nv12_rgba_us=0 {}x{}",
                    self.spec.codec, incoming_sec, decoded_sec, feed_started.elapsed().as_micros(), submit_us,
                    max_acquire_us, max_copy_us, pack_us, direct_pack, pool_reuses, pool_allocations,
                    decoder.width, decoder.height);
            }
        }
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        self.shared.frame.lock().map_err(|_| anyhow!("poisoned frame slot"))?.clear();
        self.av1_sequence_seen.store(false, Ordering::Relaxed);
        let locked = self.inner.lock().map_err(|_| anyhow!("decoder mutex poisoned"))?;
        if self.codec == CODEC_H264_NATIVE {
            self.h264_bootstrap.lock()
                .map_err(|_| anyhow!("H.264 bootstrap mutex poisoned"))?.invalidate();
        }
        if let Some(decoder) = locked.as_ref() {
            // SAFETY: serial FFI call, decoder still owns every frame.
            ffi::check("videoDecoderFlush", unsafe { ffi::pf2_decoder_flush(decoder.handle) })?;
        }
        Ok(())
    }
    fn queue_depth(&self) -> Option<u32> {
        self.shared.frame.lock().ok().map(|queue| u32::from(queue.len()))
    }
}

#[cfg(test)]
mod decoder_max_sample_tests {
    use super::sdk_max_sample_bytes;

    #[test]
    fn sdk_resource_hint_matches_switch2_decoder_tiers() {
        assert_eq!(sdk_max_sample_bytes(720, 8), 12 << 20);
        assert_eq!(sdk_max_sample_bytes(1080, 8), 12 << 20);
        assert_eq!(sdk_max_sample_bytes(1440, 8), 12 << 20);
        assert_eq!(sdk_max_sample_bytes(1080, 10), 1 << 20);
        assert_eq!(sdk_max_sample_bytes(1440, 10), 4 << 20);
        assert_eq!(sdk_max_sample_bytes(2160, 10), 4 << 20);
    }
}

#[cfg(test)]
mod av1_sequence_header_start_tests {
    use super::av1_has_sequence_header;

    #[test]
    fn delta_au_from_switch_log_has_no_sequence_header() {
        let mut au = vec![0x12, 0x00, 0x32, 0xf0, 0xea, 0x02];
        au.resize(46_454, 0);
        assert!(!av1_has_sequence_header(&au));
    }

    #[test]
    fn sequence_header_in_complete_au_is_accepted() {
        assert!(av1_has_sequence_header(&[0x12, 0x00, 0x0a, 0x02, 1, 2, 0x32, 1, 3]));
        assert!(av1_has_sequence_header(&[0x0a, 0x01, 0xaa]));
    }

    #[test]
    fn truncated_and_malformed_obus_are_rejected() {
        for au in [&[0x12, 0x00, 0x0a, 5, 0xaa][..],
                   &[0x0a, 0x80][..], &[0x82, 0][..],
                   &[0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff][..]] {
            assert!(!av1_has_sequence_header(au));
        }
    }
}
