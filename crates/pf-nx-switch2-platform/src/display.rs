//! Official Punktfunk UI: Ganesh/Vulkan composition for UI and NV12 video.
//! The original raster presentation remains available as a safe fallback.
use std::ptr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use pf_nx_app::platform::{Display, Fit, FrameReport, OperationMode, Refresh, Rgba, VideoE2e};
use skia_safe::{AlphaType, Color, ColorType, ImageInfo, Paint, Rect, SamplingOptions};

use crate::ffi;
use crate::nv12_pool::Nv12Pool;

pub(crate) struct VideoPicture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    // NV12 is packed as Y (w*h), then interleaved UV (w*h/2).
    // rgba is filled lazily ONLY when native GPU composition is unavailable.
    pub(crate) rgba: Vec<u8>,
    pub(crate) nv12: Vec<u8>,
    pub(crate) nv12_pool: Arc<Nv12Pool>,
    pub(crate) bit_depth: u8,
    pub(crate) pq: bool,
    pub(crate) matrix: i32,
    pub(crate) full_range: i32,
    pub(crate) capture_pts_ns: u64,
    pub(crate) decoded_at: Instant,
    // Monotonic instant at which the complete, copied image became available.
    pub(crate) published_at: Instant,
    pub(crate) video_e2e: VideoE2e,
}

// Only after Skia has completed its synchronous upload, and after the
// display no longer needs the frame, can its owned NV12/P010 allocation be
// reused by the decoder thread. The pool's Arc may outlive a video session.
impl Drop for VideoPicture {
    fn drop(&mut self) {
        self.nv12_pool.give_back(std::mem::take(&mut self.nv12));
    }
}

// A/B selection is embedded into Rust at compile time. In the ordinary
// (unset) build, keep the negotiated stream's smoothing policy unchanged.
// Explicit 0/1 OVERRIDES that policy so an A/B test can actually compare
// newest-wins with the bounded two-picture handoff at AV1 120 fps.
#[inline]
fn select_bounded_video_handoff(override_value: Option<&str>, smoothing: usize) -> bool {
    match override_value {
        Some("0") => false,
        Some("1") => true,
        _ => smoothing > 0,
    }
}

pub(crate) fn bounded_video_handoff(smoothing: usize) -> bool {
    select_bounded_video_handoff(option_env!("PF2_BOUNDED_PACING"), smoothing)
}

pub(crate) fn pacing_override_name() -> &'static str {
    match option_env!("PF2_BOUNDED_PACING") {
        Some("0") => "newest-wins",
        Some("1") => "two-frame",
        _ => "stream-policy",
    }
}

// The decoder owns copied NV12 bytes, never VkImages. A two-element handoff
// has no waits and no allocation of queue nodes: one picture is presented
// immediately; a second can survive a short decode burst. When full, the
// oldest is evicted so a stalled display cannot accumulate latency.
pub(crate) struct PendingPictures<T> {
    older: Option<T>,
    newest: Option<T>,
}

impl<T> Default for PendingPictures<T> {
    fn default() -> Self { Self { older: None, newest: None } }
}

impl<T> PendingPictures<T> {
    pub(crate) fn len(&self) -> u8 {
        u8::from(self.older.is_some()) + u8::from(self.newest.is_some())
    }

    pub(crate) fn push(&mut self, frame: T, bounded: bool) -> Option<T> {
        if !bounded {
            // This branch matches the previous single-slot behavior exactly.
            debug_assert!(self.older.is_none());
            return self.newest.replace(frame);
        }
        if self.newest.is_none() {
            self.newest = Some(frame);
            return None;
        }
        if self.older.is_none() {
            self.older = self.newest.take();
            self.newest = Some(frame);
            return None;
        }
        let superseded = self.older.take();
        self.older = self.newest.take();
        self.newest = Some(frame);
        superseded
    }

    fn oldest(&self) -> Option<&T> {
        self.older.as_ref().or(self.newest.as_ref())
    }

    fn take_oldest(&mut self) -> Option<T> {
        self.older.take().or_else(|| self.newest.take())
    }

    fn discard_oldest(&mut self) -> Option<T> {
        self.take_oldest()
    }

    pub(crate) fn clear(&mut self) {
        self.older = None;
        self.newest = None;
    }
}

#[derive(Default)]
pub(crate) struct VideoSlot {
    pub(crate) frame: Mutex<PendingPictures<VideoPicture>>,
    // Explicit session policy: 0 = latest only; 1..=3 = at most 2 frames.
    // This is a bounded handoff, NOT an unbounded multi-frame FIFO.
    pub(crate) smoothing_enabled: AtomicBool,
    // Count overwritten and expired images, not missing network AUs.
    pub(crate) replaced: AtomicU32,
}

#[derive(Default)]
struct PhaseMeasure {
    samples: u32,
    sum_us: u64,
    max_us: u32,
}

impl PhaseMeasure {
    fn note(&mut self, elapsed: Duration) {
        let us = elapsed.as_micros().min(u128::from(u32::MAX)) as u32;
        self.samples = self.samples.saturating_add(1);
        self.sum_us = self.sum_us.saturating_add(u64::from(us));
        self.max_us = self.max_us.max(us);
    }

    fn avg(&self) -> u64 {
        if self.samples == 0 { 0 } else { self.sum_us / u64::from(self.samples) }
    }
}

// One timestamp sample per *successful* native present. This measures the
// actual cadence of display submissions independently of the decoder frame
// rate and of scene changes. A gap alone cannot identify the source of a
// visible hitch (host, network, app scheduling or scanout).
#[derive(Clone, Copy, Default)]
struct PresentCadence {
    last_tick: Option<Instant>,
    samples: u32,
    over_1p5x: u32,
    over_2x: u32,
    max_us: u32,
}

impl PresentCadence {
    fn note(&mut self, now: Instant, refresh_hz: u32) {
        if let Some(previous) = self.last_tick {
            let us = now.saturating_duration_since(previous)
                .as_micros().min(u128::from(u32::MAX)) as u32;
            let period_us = 1_000_000u64 / u64::from(refresh_hz.max(1));
            self.samples = self.samples.saturating_add(1);
            self.max_us = self.max_us.max(us);
            if u64::from(us).saturating_mul(2) >= period_us.saturating_mul(3) {
                self.over_1p5x = self.over_1p5x.saturating_add(1);
            }
            if u64::from(us) >= period_us.saturating_mul(2) {
                self.over_2x = self.over_2x.saturating_add(1);
            }
        }
        self.last_tick = Some(now);
    }

    fn take_samples(&mut self) -> Self {
        let samples = *self;
        *self = Self { last_tick: samples.last_tick, ..Self::default() };
        samples
    }
}

#[derive(Default)]
struct FramePhase {
    publish_to_pull: PhaseMeasure,
    pull_to_submit: PhaseMeasure,
    pull_to_return: PhaseMeasure,
    backlog_ticks: u32,
    expired: u32,
    max_pending: u8,
}

/// Offscreen raster surface composited with decoded video over the same display.
pub struct NativeDisplay {
    presenter: *mut ffi::Handle,
    surface: skia_safe::Surface,
    size: (u32, u32),
    // Current window output (may change on a TV resolution change while docked).
    window_size: (u32, u32),
    ui_enabled: bool,
    stream_enabled: bool,
    fit: Fit,
    video: Arc<VideoSlot>,
    current: Option<VideoPicture>,
    pixels: Vec<u8>,
    // Video is rescaled only when a NEW decoded picture arrives; UI can
    // continue to animate at 120 Hz without rescaling an unchanged picture.
    video_pixels: Vec<u8>,
    video_cache_valid: bool,
    docked: bool,
    mode: Arc<AtomicBool>,
    frame_number: u64,
    refresh_hz: u32,
    // Ganesh shares the native presenter Vulkan device; it must be dropped first.
    gpu_context: Option<skia_safe::gpu::DirectContext>,
    gpu_surface: Option<skia_safe::Surface>,
    gpu_video_shader: Option<skia_safe::Shader>,
    // Compiled SkSL is reused across pictures with the same geometry/color.
    gpu_video_effect: Option<(VideoEffectKey, skia_safe::runtime_effect::RuntimeEffect)>,
    stream_display_ticks: u32,
    stream_display_fresh: u32,
    stream_display_reused: u32,
    stream_last_stats: Instant,
    phase: FramePhase,
    present_cadence: PresentCadence,
    pulled_at: Option<Instant>,
    // Native present can copy the same, already flushed TRANSFER_SRC image on
    // ticks without a new video frame. Never reuse when the UI changed.
    gpu_copy_ready: bool,
    gpu_disabled: bool,
    gpu_queue_family: u32,
    pending_window: Option<((u32, u32), Instant)>,
    // Do not rebuild Vulkan for a one-poll dock/undock transition.
    pending_mode: Option<(bool, Instant)>,
    tv_policy: (u32, u32, u32),
    resize_pending: bool,
    last_frame_gpu: bool,
    ganesh_probe_done: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VideoEffectKey {
    source: (u32, u32),
    target: (u32, u32),
    fit: Fit,
    matrix: i32,
    full_range: i32,
    bit_depth: u8,
    pq: bool,
}

// Reuse an offscreen Vulkan image only while its contents remain valid. The
// presenter still copies/presents it each tick, keeping FIFO/vblank pacing;
// no Skia command list, texture upload or GPU synchronization is needed when
// both the picture and the overlay are unchanged.
fn reuse_unchanged_video(stream: bool, ui: bool, fresh: bool, ready: bool) -> bool {
    stream && !ui && !fresh && ready
}

// The NV12 bytes are uploaded as one Gray8 raster (Y followed by UV rows).
// The child shader MUST use nearest sampling: the chroma plane interleaves
// U,V bytes and hardware bilinear filtering would mix the two channels.
// Perform reconstruction explicitly: area-integrated luma on moderate
// downscales (e.g. 2560x1440 -> 1920x1080), bilinear luma otherwise, and
// bilinear U/V independently at the correct 4:2:0 sample locations.
fn make_video_sksl(key: VideoEffectKey) -> String {
    if key.bit_depth == 10 { return make_p010_sksl(key); }
    let (sw, sh) = (key.source.0 as f32, key.source.1 as f32);
    let (dw, dh) = (key.target.0 as f32, key.target.1 as f32);
    let (sx, sy) = match key.fit {
        Fit::Stretch => (dw / sw, dh / sh),
        Fit::Contain => { let s=(dw/sw).min(dh/sh); (s,s) },
        Fit::Cover => { let s=(dw/sw).max(dh/sh); (s,s) },
    };
    let (ox, oy) = ((dw - sw*sx)*0.5, (dh - sh*sy)*0.5);
    // The box covers at most three source pixels per axis if scale >= 0.5.
    // For stronger minification, bilinear is a bounded-cost fallback; a
    // dedicated multilevel downsampler would be required for ideal 4K->1080p.
    let luma_lookup = if sx >= 0.5 && sy >= 0.5 && (sx < 1.0 || sy < 1.0) {
        "luma_box3(q)"
    } else {
        "luma_bilinear(q)"
    };
    let (cr, cb, gu, gv) = if key.full_range != 0 {
        if key.matrix == 1 { (403,475,48,120) } else { (359,454,88,183) }
    } else if key.matrix == 1 { (459,541,55,136) }
      else { (409,516,100,208) };
    let yscale = if key.full_range != 0 { 1.0 } else { 298.0/256.0 };
    let yoffset = if key.full_range != 0 { 0.0 } else { 16.0 };
    // Geometry and color parameters are compiler constants to avoid uniform
    // packing assumptions in skia-safe 0.153.3. Compile only when they change.
    format!(r#"
        uniform shader nv12_plane;
        float y_texel(float2 index) {{
            float2 i = clamp(index, float2(0.0, 0.0), float2({sw_max:.1}, {sh_max:.1}));
            return nv12_plane.eval(i + float2(0.5, 0.5)).r * 255.0;
        }}
        float luma_bilinear(float2 q) {{
            float2 t = q - float2(0.5, 0.5);
            float2 b = floor(t);
            float2 f = t - b;
            float a = mix(y_texel(b), y_texel(b + float2(1.0, 0.0)), f.x);
            float c = mix(y_texel(b + float2(0.0, 1.0)),
                          y_texel(b + float2(1.0, 1.0)), f.x);
            return mix(a, c, f.y);
        }}
        float luma_box3(float2 q) {{
            // Integrate each output pixel's source-space footprint exactly.
            // For a 0.75x shrink this is a 1.333 x 1.333 source-pixel box.
            float2 halfbox = float2({inv_sx_half:.8}, {inv_sy_half:.8});
            float2 lo = q - halfbox;
            float2 hi = q + halfbox;
            float2 b = floor(lo);
            float3 wx = float3(
                max(0.0, min(hi.x, b.x + 1.0) - max(lo.x, b.x)),
                max(0.0, min(hi.x, b.x + 2.0) - max(lo.x, b.x + 1.0)),
                max(0.0, min(hi.x, b.x + 3.0) - max(lo.x, b.x + 2.0)));
            float3 wy = float3(
                max(0.0, min(hi.y, b.y + 1.0) - max(lo.y, b.y)),
                max(0.0, min(hi.y, b.y + 2.0) - max(lo.y, b.y + 1.0)),
                max(0.0, min(hi.y, b.y + 3.0) - max(lo.y, b.y + 2.0)));
            float r0 = wx.x*y_texel(b) + wx.y*y_texel(b + float2(1.0, 0.0))
                     + wx.z*y_texel(b + float2(2.0, 0.0));
            float r1 = wx.x*y_texel(b + float2(0.0, 1.0))
                     + wx.y*y_texel(b + float2(1.0, 1.0))
                     + wx.z*y_texel(b + float2(2.0, 1.0));
            float r2 = wx.x*y_texel(b + float2(0.0, 2.0))
                     + wx.y*y_texel(b + float2(1.0, 2.0))
                     + wx.z*y_texel(b + float2(2.0, 2.0));
            return (wy.x*r0 + wy.y*r1 + wy.z*r2) /
                   (4.0*halfbox.x*halfbox.y);
        }}
        float chroma_texel(float2 index, float component) {{
            float2 i = clamp(index, float2(0.0, 0.0),
                             float2({chroma_w_max:.1}, {chroma_h_max:.1}));
            // One UV pair occupies two consecutive bytes in the gray texture.
            return nv12_plane.eval(float2(2.0*i.x + 0.5 + component,
                                         {sh:.1} + i.y + 0.5)).r*255.0 - 128.0;
        }}
        float2 chroma_bilinear(float2 q) {{
            // Chroma centers lie at source coordinates (1,1), (3,1), ...
            float2 t = q*0.5 - float2(0.5, 0.5);
            float2 b = floor(t);
            float2 f = t - b;
            float2 a = float2(chroma_texel(b, 0.0), chroma_texel(b, 1.0));
            float2 c = float2(chroma_texel(b + float2(1.0, 0.0), 0.0),
                              chroma_texel(b + float2(1.0, 0.0), 1.0));
            float2 d = float2(chroma_texel(b + float2(0.0, 1.0), 0.0),
                              chroma_texel(b + float2(0.0, 1.0), 1.0));
            float2 e = float2(chroma_texel(b + float2(1.0, 1.0), 0.0),
                              chroma_texel(b + float2(1.0, 1.0), 1.0));
            return mix(mix(a,c,f.x), mix(d,e,f.x), f.y);
        }}
        half4 main(float2 p) {{
            float2 q = (p - float2({ox:.8}, {oy:.8})) / float2({sx:.8}, {sy:.8});
            if (q.x < 0.0 || q.y < 0.0 || q.x >= {sw:.1} || q.y >= {sh:.1})
                return half4(0.0, 0.0, 0.0, 1.0);
            float yy = {luma_lookup};
            float2 uv = chroma_bilinear(q);
            float uu = uv.x;
            float vv = uv.y;
            float base_y = max(yy - {yoffset:.1}, 0.0) * {yscale:.8};
            float rr = base_y + vv * {cr:.8};
            float gg = base_y - uu * {gu:.8} - vv * {gv:.8};
            float bb = base_y + uu * {cb:.8};
            return half4(half3(clamp(float3(rr,gg,bb) / 255.0, 0.0, 1.0)), 1.0);
        }}
    "#,
      sw_max = sw - 1.0, sh_max = sh - 1.0,
      chroma_w_max = sw*0.5 - 1.0, chroma_h_max = sh*0.5 - 1.0,
      inv_sx_half = 0.5/sx, inv_sy_half = 0.5/sy,
      cr=cr as f32 / 256.0, cb=cb as f32 / 256.0,
      gu=gu as f32 / 256.0, gv=gv as f32 / 256.0)
}

// P010 is 16-bit little-endian 4:2:0 with 10 significant bits in the
// most-significant bits. The uploaded Gray8 image is 2w x 3h/2 bytes; each
// u16 sample is reconstructed in the GPU shader from two exact byte fetches.
// PQ/BT.2020 is converted to linear light and tone-mapped to SDR, because the
// existing OunceVK scanout surface is RGBA8. This is *not* native HDR10 output.
fn make_p010_sksl(key: VideoEffectKey) -> String {
    let (sw, sh) = (key.source.0 as f32, key.source.1 as f32);
    let (dw, dh) = (key.target.0 as f32, key.target.1 as f32);
    let (sx, sy) = match key.fit {
        Fit::Stretch => (dw/sw, dh/sh),
        Fit::Contain => { let scale=(dw/sw).min(dh/sh); (scale, scale) },
        Fit::Cover => { let scale=(dw/sw).max(dh/sh); (scale, scale) },
    };
    let (ox, oy) = ((dw - sw*sx)*0.5, (dh - sh*sy)*0.5);
    let (ymin, yrange, crange) = if key.full_range != 0 {
        (0.0, 1023.0, 1023.0)
    } else {
        (64.0, 876.0, 896.0)
    };
    let (vr, ug, vg, ub) = if key.matrix == 9 {
        (1.4746, 0.164553, 0.571353, 1.8814) // BT.2020 NCL
    } else if key.matrix == 1 {
        (1.5748, 0.187324, 0.468124, 1.8556) // BT.709
    } else {
        (1.402, 0.344136, 0.714136, 1.772)  // BT.601
    };
    let color = if key.pq { r#"
            // ST.2084 absolute EOTF, 0..10,000 nits. Roll off into an
            // SDR 203-nit reference-white target, then convert primaries.
            float3 e = clamp(rgb, 0.0, 1.0);
            float3 p = pow(e, float3(1.0 / 78.84375));
            float3 nits = pow(max(p - 0.8359375, 0.0) /
                max(18.8515625 - 18.6875*p, 0.00001),
                float3(1.0 / 0.1593017578)) * 10000.0;
            float3 lin = nits / (nits + 203.0);
            float3 bt709 = float3(
                1.6605*lin.r - 0.5876*lin.g - 0.0728*lin.b,
               -0.1246*lin.r + 1.1329*lin.g - 0.0083*lin.b,
               -0.0182*lin.r - 0.1006*lin.g + 1.1187*lin.b);
            rgb = pow(clamp(bt709, 0.0, 1.0), float3(1.0 / 2.2));
"# } else { "" };
    format!(r#"
        uniform shader nv12_plane;
        float word_at(float bx, float by) {{
            float lo = nv12_plane.eval(float2(bx + 0.5, by + 0.5)).r * 255.0;
            float hi = nv12_plane.eval(float2(bx + 1.5, by + 0.5)).r * 255.0;
            return (floor(lo + 0.5) + 256.0*floor(hi + 0.5)) / 64.0;
        }}
        half4 main(float2 p) {{
            float2 q = (p - float2({ox:.8}, {oy:.8})) / float2({sx:.8}, {sy:.8});
            if (q.x < 0.0 || q.y < 0.0 || q.x >= {sw:.1} || q.y >= {sh:.1})
                return half4(0.0, 0.0, 0.0, 1.0);
            float yy = word_at(floor(q.x)*2.0, floor(q.y));
            float cx = floor(q.x*0.5)*4.0;
            float cy = {sh:.1} + floor(q.y*0.5);
            float uu = word_at(cx,cy);
            float vv = word_at(cx+2.0,cy);
            float y = clamp((yy - {ymin:.1})/{yrange:.1},0.0,1.0);
            float u = (uu - 512.0)/{crange:.1};
            float v = (vv - 512.0)/{crange:.1};
            float3 rgb = float3(y+{vr:.8}*v,
                y-{ug:.8}*u-{vg:.8}*v, y+{ub:.8}*u);
            {color}
            return half4(half3(clamp(rgb,0.0,1.0)),1.0);
        }}
    "#)
}

// Raster fallback is used only when Ganesh fails. Preserve the 10-bit plane
// sizes and show a color-managed image rather than misinterpreting P010 as NV12.
fn p010_raster(frame: &VideoPicture) -> Result<Vec<u8>> {
    let (w,h) = (frame.width as usize, frame.height as usize);
    let required = w.checked_mul(h).and_then(|n| n.checked_mul(3))
        .context("P010 dimensions overflow")?;
    anyhow::ensure!(frame.bit_depth == 10 && frame.nv12.len() == required,
        "invalid P010 raster fallback input");
    let mut rgba = vec![0u8; w*h*4];
    let sample = |byte_index: usize| -> f32 {
        u16::from_le_bytes([frame.nv12[byte_index],frame.nv12[byte_index+1]])
            .wrapping_shr(6) as f32
    };
    let (ymin, yrange, crange) = if frame.full_range != 0 {
        (0.0_f32,1023.0_f32,1023.0_f32)
    } else { (64.0,876.0,896.0) };
    let (vr,ug,vg,ub) = match frame.matrix {
        9 => (1.4746,0.164553,0.571353,1.8814),
        1 => (1.5748,0.187324,0.468124,1.8556),
        _ => (1.402,0.344136,0.714136,1.772),
    };
    let tonemap = |pq: f32| -> f32 {
        if !frame.pq { return pq.clamp(0.0,1.0); }
        let p = pq.clamp(0.0,1.0).powf(1.0/78.84375);
        let nits = ((p-0.8359375).max(0.0) /
            (18.8515625-18.6875*p).max(0.00001)).powf(1.0/0.1593017578)*10_000.0;
        (nits/(nits+203.0)).clamp(0.0,1.0)
    };
    for row in 0..h {
        for col in 0..w {
            let y = ((sample((row*w+col)*2)-ymin)/yrange).clamp(0.0,1.0);
            let uv = w*h*2+(row/2*w+(col/2)*2)*2;
            let u=(sample(uv)-512.0)/crange;
            let v=(sample(uv+2)-512.0)/crange;
            let rgb=[y+vr*v,y-ug*u-vg*v,y+ub*u];
            let rgb=if frame.pq {
                let lin=rgb.map(tonemap);
                [1.6605*lin[0]-0.5876*lin[1]-0.0728*lin[2],
                 -0.1246*lin[0]+1.1329*lin[1]-0.0083*lin[2],
                 -0.0182*lin[0]-0.1006*lin[1]+1.1187*lin[2]]
                    .map(|v| v.clamp(0.0,1.0).powf(1.0/2.2))
            } else { rgb };
            let dst=(row*w+col)*4;
            for i in 0..3 {
                rgba[dst+i] = (rgb[i].clamp(0.0,1.0)*255.0+0.5) as u8;
            }
            rgba[dst+3]=255;
        }
    }
    Ok(rgba)
}

impl NativeDisplay {
    pub(crate) fn new(video: Arc<VideoSlot>, mode: Arc<AtomicBool>) -> Result<Self> {
        let docked = mode.load(Ordering::Acquire);
        let (presenter, size, window_size) = Self::open_presenter(docked)?;
        let mut hdr10_formats = 0u32;
        let hdr10_rc = unsafe { ffi::pf2_presenter_hdr10_surface(presenter, &mut hdr10_formats) };
        tracing::info!(
            "switch2 HDR10 surface probe: rc={} advertised_pq_10bit={} native_hdr_scanout_enabled=false (stream Main10 uses SDR tone map)",
            hdr10_rc, hdr10_formats != 0
        );
        tracing::info!(
            "switch2 TV v2 ACTIVE: native window follows system output; 120 Hz is requested independently of Vulkan resize"
        );
        tracing::info!(
            "switch2 frame handoff: override={} (PF2_BOUNDED_PACING=0 newest-wins, =1 two-frame, unset stream policy); stale_budget=one_refresh",
            pacing_override_name(),
        );
        // The Vulkan/Ganesh path must not preallocate a full-resolution CPU
        // framebuffer. At 4K, three RGBA buffers would exceed 95 MiB before
        // any decoder or swapchain images are accounted for.
        let surface = match Self::raster((1, 1)) {
            Ok(s) => s,
            Err(e) => {
                // SAFETY: presenter is owned by this constructor and not shared.
                unsafe { ffi::pf2_presenter_close(presenter) };
                return Err(e);
            }
        };
        // Compile-time, opt-in scanout check independent of Ganesh and fonts.
        // Must not alter normal release startup or the official frontend.
        if matches!(option_env!("PF2_SCANOUT_PROBE"), Some("1")) {
            if let Err(error) = Self::diagnostic_scanout(presenter, size) {
                tracing::warn!("switch2 diagnostic scanout failed: {error:#}");
            }
        }
        Ok(Self { presenter, surface, size, window_size, ui_enabled: true,
            stream_enabled: false, fit: Fit::Contain, video,
            current: None, video_pixels: Vec::new(), video_cache_valid: false,
            pixels: Vec::new(), docked, mode, frame_number: 0, refresh_hz: 60,
            gpu_context: None, gpu_surface: None, gpu_video_shader: None,
            gpu_video_effect: None, stream_display_ticks: 0, stream_display_fresh: 0,
            stream_display_reused: 0, stream_last_stats: Instant::now(),
            phase: FramePhase::default(), present_cadence: PresentCadence::default(), pulled_at: None,
            gpu_copy_ready: false, gpu_disabled: false,
            gpu_queue_family: 0, pending_window: None, pending_mode: None,
            tv_policy: (0, 0, 60),
            resize_pending: false, last_frame_gpu: false, ganesh_probe_done: false })
    }

    /// A blue/green split with a centered white square. This bypasses Skia:
    /// if the screen stays black, focus on VI/window/Vulkan presentation.
    /// Enable only for diagnostics: PF2_SCANOUT_PROBE=1 ./build-switch2.sh.
    fn diagnostic_scanout(presenter: *mut ffi::Handle, size: (u32, u32)) -> Result<()> {
        let (w, h) = (size.0 as usize, size.1 as usize);
        let bytes = w.checked_mul(h).and_then(|x| x.checked_mul(4))
            .context("diagnostic scanout size overflow")?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(bytes).context("diagnostic scanout buffer")?;
        pixels.resize(bytes, 0);
        for y in 0..h {
            for x in 0..w {
                let p = (y*w+x)*4;
                let square = x >= w*3/8 && x < w*5/8 && y >= h*3/8 && y < h*5/8;
                let rgb = if square { [255, 255, 255] }
                    else if x < w/2 { [0, 60, 255] } else { [0, 220, 80] };
                pixels[p..p+3].copy_from_slice(&rgb);
                pixels[p+3] = 255;
            }
        }
        // SAFETY: data stays alive and the presenter owns the window.
        ffi::check("OunceVK diagnostic scanout", unsafe {
            ffi::pf2_presenter_frame(presenter, pixels.as_ptr(), bytes, w*4)
        })?;
        tracing::warn!("switch2 diagnostic scanout presented: {}x{} blue/green + white center; normal UI follows", w,h);
        std::thread::sleep(Duration::from_millis(300));
        Ok(())
    }

    fn update_refresh(&mut self) {
        let (mut hz, mut policy_rc) = (60u32, 0u32);
        // SAFETY: borrowed VI2 state belongs to the live OunceVK default window.
        let rc = unsafe { ffi::pf2_presenter_refresh(self.presenter, &mut hz, &mut policy_rc) };
        if rc == ffi::OK && (hz == 60 || hz == 120) {
            if hz != self.refresh_hz || self.frame_number == 1 {
                tracing::info!("switch2 VI2 refresh: nominal_hz={} request_rc=0x{:x} query_rc={} (decoder configuration independent of stream fps)", hz, policy_rc, rc);
            }
            self.refresh_hz = hz;
        } else if self.frame_number == 1 {
            tracing::warn!("switch2 VI2 refresh unavailable: rc={} request_rc=0x{:x}; keeping {} Hz", rc, policy_rc, self.refresh_hz);
        }
    }

    fn raster(size: (u32, u32)) -> Result<skia_safe::Surface> {
        let info = ImageInfo::new(
            (size.0 as i32, size.1 as i32), ColorType::RGBA8888, AlphaType::Premul, None);
        skia_safe::surfaces::raster(&info, None, None).ok_or_else(|| anyhow!("Skia raster allocation"))
    }

    fn open_presenter(docked: bool) -> Result<(*mut ffi::Handle, (u32, u32), (u32, u32))> {
        // C selects the actual native window size; these are fallback hints.
        let (w, h) = (1920, 1080);
        let mut presenter = ptr::null_mut();
        // SAFETY: out pointer writable and ownership transferred on successful return.
        let native_rc = unsafe { ffi::pf2_presenter_open(&mut presenter, w, h) };
        if native_rc != ffi::OK {
            tracing::error!("switch2 native display Vulkan open failed: rc={native_rc} mode={}; trying emergency 1080p layer",
                if docked { "docked" } else { "handheld" });
            // A fallback is better than returning to HBMenu on a TV mode
            // unsupported by the current driver. Log its effective extent.
            ffi::check("OunceVK 1080p emergency presenter", unsafe {
                ffi::pf2_presenter_open_fixed(&mut presenter, w, h)
            })?;
        }
        if presenter.is_null() { anyhow::bail!("presenter opened without a handle") }
        let (mut pw, mut ph) = (0, 0);
        // SAFETY: presenter is live, out values writable.
        let rc = unsafe { ffi::pf2_presenter_size(presenter, &mut pw, &mut ph) };
        if rc != ffi::OK || pw == 0 || ph == 0 || pw > 3840 || ph > 2160 {
            // SAFETY: only constructor owns this pointer here.
            unsafe { ffi::pf2_presenter_close(presenter) };
            anyhow::bail!("OunceVK swapchain extent: rc={rc} {pw}x{ph}");
        }
        let (mut native_w, mut native_h, mut policy_rc) = (0, 0, 0);
        let window_rc = unsafe {
            ffi::pf2_presenter_output_size(presenter, &mut native_w, &mut native_h, &mut policy_rc)
        };
        let window = if window_rc == ffi::OK { (native_w, native_h) } else { (pw, ph) };
        tracing::info!(
            "switch2 output mode: {} window={}x{} swapchain={}x{} policy_rc=0x{:x} window_rc={}",
            if docked { "docked" } else { "handheld" },
            window.0, window.1, pw, ph, policy_rc, window_rc,
        );
        if window != (pw, ph) {
            tracing::warn!(
                "switch2 output extent mismatch: native window {}x{} != Vulkan swapchain {}x{}; Native tracks the actual scanout extent",
                window.0, window.1, pw, ph,
            );
        }
        Ok((presenter, (pw, ph), window))
    }

    fn rebuild(&mut self, docked: bool) -> Result<()> {
        // Moonlight NX2 keeps its device/VI layer alive across mode changes.
        // Dropping and re-opening a whole presenter conflicts with the same
        // nwindowGetDefault() layer and caused the black-screen docked crash.
        tracing::info!("switch2 presenter transition: in-place swapchain resize {}x{} -> {}",
            self.size.0, self.size.1, if docked { "docked" } else { "handheld" });
        self.gpu_video_shader = None;
        self.gpu_video_effect = None;
        self.gpu_copy_ready = false;
        drop(self.gpu_surface.take());
        drop(self.gpu_context.take());
        self.last_frame_gpu = false;
        self.ganesh_probe_done = false;
        // A failed resize must be retried before touching borrowed VkImages.
        self.resize_pending = true;
        ffi::check("OunceVK in-place swapchain resize", unsafe {
            ffi::pf2_presenter_resize(self.presenter)
        })?;
        let (mut pw, mut ph) = (0, 0);
        ffi::check("OunceVK resized extent", unsafe {
            ffi::pf2_presenter_size(self.presenter, &mut pw, &mut ph)
        })?;
        if pw < 2 || ph < 2 || pw > 3840 || ph > 2160 {
            anyhow::bail!("invalid resized Vulkan extent {pw}x{ph}");
        }
        let (mut native_w, mut native_h, mut policy_rc) = (0, 0, 0);
        let rc = unsafe {
            ffi::pf2_presenter_output_size(self.presenter, &mut native_w, &mut native_h, &mut policy_rc)
        };
        self.size = (pw, ph);
        self.window_size = if rc == ffi::OK { (native_w, native_h) } else { (pw, ph) };
        self.surface = Self::raster((1, 1))?;
        self.pixels.clear();
        self.video_pixels.clear();
        self.video_cache_valid = false;
        self.stream_display_ticks = 0;
        self.stream_display_fresh = 0;
        self.stream_display_reused = 0;
        self.stream_last_stats = Instant::now();
        self.phase = FramePhase::default();
        self.present_cadence = PresentCadence::default();
        self.pulled_at = None;
        self.docked = docked;
        self.pending_window = None;
        self.pending_mode = None;
        self.resize_pending = false;
        self.refresh_hz = 60;
        tracing::info!("switch2 presenter resized: mode={} swapchain={}x{} window={}x{} size_policy_rc=0x{:x} request={}x{}@{}",
            if docked { "docked" } else { "handheld" }, pw, ph,
            self.window_size.0, self.window_size.1, policy_rc,
            self.tv_policy.0, self.tv_policy.1, self.tv_policy.2);
        if policy_rc != 0 {
            tracing::warn!("switch2 TV size request was refused by Horizon (rc=0x{policy_rc:x}); previous mode retained");
        }
        self.update_refresh();
        Ok(())
    }

    fn prepare_raster(&mut self) -> Result<()> {
        // Normally never called: Ganesh composites video and the official UI.
        // Keep a recoverable CPU fallback but don't reserve ~100 MiB of CPU
        // buffers at 4K during ordinary GPU playback.
        let bytes = (self.size.0 as usize)
            .checked_mul(self.size.1 as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .context("raster framebuffer size overflow")?;
        // One framebuffer and Skia canvas are always needed. The third RGBA
        // plane is required only for the *video + changing UI* case: video
        // only can remain directly in pixels across 120 Hz display ticks.
        // Above 1080p prefer recovery that fits in RAM: compositing video
        // over a changing UI requires re-scaling per frame, but avoids the
        // extra 32 MiB RGBA cache at 4K. Raster is emergency-only at 4K.
        let needs_video_cache = self.stream_enabled && self.ui_enabled &&
            u64::from(self.size.0)*u64::from(self.size.1) <= 1920*1080;
        if self.pixels.len() == bytes &&
           (self.video_pixels.len() == bytes) == needs_video_cache &&
           self.surface.width() == self.size.0 as i32 &&
           self.surface.height() == self.size.1 as i32 {
            return Ok(());
        }
        tracing::warn!("switch2 raster fallback: {}x{} primary={} MiB video_cache={} MiB",
            self.size.0, self.size.1, bytes / (1024 * 1024),
            if needs_video_cache { bytes / (1024 * 1024) } else { 0 });
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(bytes).context("allocating CPU RGBA framebuffer")?;
        pixels.resize(bytes, 0);
        let mut video_pixels = Vec::new();
        if needs_video_cache {
            video_pixels.try_reserve_exact(bytes).context("allocating CPU video framebuffer")?;
            video_pixels.resize(bytes, 0);
        }
        let surface = Self::raster(self.size)?;
        self.pixels = pixels;
        self.video_pixels = video_pixels;
        self.surface = surface;
        self.video_cache_valid = false;
        Ok(())
    }

    fn probe_ganesh(&mut self) {
        use skia_safe::gpu;

        self.gpu_copy_ready = false;
        self.ganesh_probe_done = true;
        let mut vk = ffi::VkBorrowedContext {
            instance: ptr::null_mut(), physical_device: ptr::null_mut(),
            device: ptr::null_mut(), queue: ptr::null_mut(),
            queue_family: 0, physical_api_version: 0,
        };
        // SAFETY: the presenter remains owned by NativeDisplay for the duration
        // of the probe; returned Vulkan handles are borrowed, not adopted.
        let vk_rc = unsafe { ffi::pf2_presenter_vk_borrow(self.presenter, &mut vk) };
        if vk_rc != ffi::OK {
            tracing::warn!("switch2 ganesh preflight: cannot borrow Vulkan context (rc={vk_rc}); raster unchanged");
            return;
        }
        self.gpu_queue_family = vk.queue_family;
        tracing::info!("switch2 ganesh preflight: OunceVK handles borrowed; queue_family={} physical_api={}.{}.{} (raster unchanged)",
            vk.queue_family, vk.physical_api_version >> 22,
            (vk.physical_api_version >> 12) & 1023,
            vk.physical_api_version & 4095);

        // pf2_presenter_open() currently requests Vulkan API 1.3 even when
        // vkGetPhysicalDeviceProperties reports support for 1.4. Never tell
        // Skia that a higher API version was requested for this instance.
        const INSTANCE_API_VERSION: u32 = (1 << 22) | (3 << 12);
        let get_proc = |of| match of {
            gpu::vk::GetProcOf::Instance(instance, name) => {
                // SAFETY: proc pointers are looked up through the live
                // Vulkan instance owned by the same presenter.
                unsafe { ffi::pf2_vk_instance_proc(instance as *mut _, name.cast::<core::ffi::c_char>()) }
            }
            gpu::vk::GetProcOf::Device(device, name) => {
                // SAFETY: device and function name come from Skia's loader
                // and refer to the existing OunceVK device.
                unsafe { ffi::pf2_vk_device_proc(device as *mut _, name.cast::<core::ffi::c_char>()) }
            }
        };
        // SAFETY: the Vulkan handles stay live until NativeDisplay destroys
        // gpu_context, always before pf2_presenter_close() / device teardown.
        let ctx = unsafe {
            let backend = gpu::vk::BackendContext::new_builder(
                vk.instance as _,
                vk.physical_device as _,
                vk.device as _,
                (vk.queue as _, vk.queue_family as usize),
                &get_proc,
                Some(INSTANCE_API_VERSION.into()),
            ).build();
            gpu::ganesh::vk::direct_contexts::make_vulkan(&backend, None)
        };
        if let Some(ctx) = ctx {
            self.gpu_context = Some(ctx);
            tracing::info!("switch2 ganesh context: Vulkan DirectContext created (GPU surface pending)");
            if let Err(error) = self.open_gpu_surface() {
                self.gpu_disabled = true;
                tracing::warn!("switch2 ganesh surface: {error:#}; raster remains active");
            }
        } else {
            self.gpu_disabled = true;
            tracing::warn!("switch2 ganesh context: initialization failed; raster remains active");
        }
    }

    fn open_gpu_surface(&mut self) -> Result<()> {
        use skia_safe::gpu;
        let mut offscreen = ffi::VkOffscreenImage::default();
        ffi::check("OunceVK offscreen image", unsafe {
            ffi::pf2_presenter_gpu_image(self.presenter, &mut offscreen)
        })?;
        if (offscreen.width, offscreen.height) != self.size || offscreen.image == 0 {
            anyhow::bail!("incompatible offscreen extent or null VkImage");
        }
        let (format, color_type) = match offscreen.format {
            37 => (gpu::vk::Format::R8G8B8A8_UNORM, ColorType::RGBA8888),
            44 => (gpu::vk::Format::B8G8R8A8_UNORM, ColorType::BGRA8888),
            other => anyhow::bail!("unsupported Vulkan offscreen format {other}"),
        };
        // SAFETY: the image and VkDeviceMemory are owned by the native presenter;
        // its lifetime exceeds both the wrapped Surface and the DirectContext.
        let mut info = unsafe { gpu::vk::ImageInfo::new(
            offscreen.image as _, gpu::vk::Alloc::default(),
            gpu::vk::ImageTiling::OPTIMAL, gpu::vk::ImageLayout::UNDEFINED,
            format, 1, Some(offscreen.queue_family), None, None, None,
        ) };
        // Ganesh check_image_info() requires both transfer usage bits, even
        // when this externally owned render target is only copied FROM here.
        // Vulkan: COLOR_ATTACHMENT (0x10) | TRANSFER_SRC (0x01) | TRANSFER_DST (0x02).
        // Keep in sync with pf2_presenter_gpu_image()'s VkImageCreateInfo::usage.
        info.image_usage_flags = 0x10 | 0x01 | 0x02;
        let target = gpu::backend_render_targets::make_vk(
            (offscreen.width as i32, offscreen.height as i32), &info);
        tracing::info!("switch2 ganesh surface candidate: {}x{} format={} usage=0x{:x} queue_family={}",
            offscreen.width, offscreen.height, offscreen.format,
            info.image_usage_flags, offscreen.queue_family);
        let context = self.gpu_context.as_mut().context("Ganesh DirectContext missing")?;
        self.gpu_surface = gpu::surfaces::wrap_backend_render_target(
            context, &target, gpu::SurfaceOrigin::TopLeft, color_type, None, None);
        if self.gpu_surface.is_none() {
            anyhow::bail!("wrap_backend_render_target returned None (VkFormat={}, usage=0x{:x}, queue_family={})",
                offscreen.format, info.image_usage_flags, offscreen.queue_family);
        }
        tracing::info!("switch2 ganesh surface: created {}x{} format={} usage=COLOR_ATTACHMENT|TRANSFER_SRC|TRANSFER_DST samples=1 queue_family={}",
            offscreen.width, offscreen.height, offscreen.format, offscreen.queue_family);
        Ok(())
    }

    fn pull_video(&mut self) -> Result<bool> {
        self.pulled_at = None;
        if self.stream_enabled {
            // No timed wait and no new scheduling thread. The normal path
            // consumes the only pending image on this same display tick.
            let (frame, expired, pending_before) = {
                let mut queue = self.video.frame.lock()
                    .map_err(|_| anyhow!("poisoned video slot"))?;
                let pending_before = queue.len();
                let period_us = 1_000_000u64 / u64::from(self.refresh_hz.max(1));
                // Never display a burst's older image after it has already
                // waited for a complete refresh. This bounds extra latency
                // even when the incoming clock jitters or the display stalls.
                let expired = if self.video.smoothing_enabled.load(Ordering::Relaxed) && pending_before == 2 &&
                    queue.oldest().is_some_and(|picture|
                        picture.published_at.elapsed() >= Duration::from_micros(period_us)) {
                    queue.discard_oldest()
                } else { None };
                let frame = queue.take_oldest();
                (frame, expired, pending_before)
            };
            // A large NV12 Vec can be released here, outside the mutex.
            if expired.is_some() {
                self.video.replaced.fetch_add(1, Ordering::Relaxed);
                self.phase.expired = self.phase.expired.saturating_add(1);
            }
            drop(expired);
            if pending_before == 2 {
                self.phase.backlog_ticks = self.phase.backlog_ticks.saturating_add(1);
            }
            self.phase.max_pending = self.phase.max_pending.max(pending_before);
            if let Some(frame) = frame {
                let pulled_at = Instant::now();
                self.phase.publish_to_pull.note(pulled_at.duration_since(frame.published_at));
                self.pulled_at = Some(pulled_at);
                self.current = Some(frame);
                self.gpu_video_shader = None;
                self.video_cache_valid = false;
                return Ok(true);
            }
        } else {
            self.current = None;
            self.gpu_video_shader = None;
            self.video.frame.lock()
                .map_err(|_| anyhow!("poisoned video slot"))?.clear();
        }
        Ok(false)
    }

    fn prepare_video_shader(&mut self) -> Result<()> {
        let picture = self.current.as_ref().context("no decoded picture")?;
        let key = VideoEffectKey {
            source: (picture.width, picture.height), target: self.size, fit: self.fit,
            matrix: picture.matrix, full_range: picture.full_range,
            bit_depth: picture.bit_depth, pq: picture.pq,
        };
        let w = picture.width as usize;
        let h = picture.height as usize;
        let bytes_per_sample = if picture.bit_depth == 10 { 2 } else { 1 };
        let row_bytes = w * bytes_per_sample;
        anyhow::ensure!(w > 0 && h > 0 && w%2==0 && h%2==0 &&
            picture.nv12.len() == row_bytes*h*3/2, "invalid packed NV12/P010 picture");
        if self.gpu_video_effect.as_ref().is_none_or(|(old, _)| *old != key) {
            let source = make_video_sksl(key);
            let effect = skia_safe::runtime_effect::RuntimeEffect::make_for_shader(&source, None)
                .map_err(|e| anyhow!("NV12 SkSL compilation: {e}"))?;
            self.gpu_video_effect = Some((key, effect));
            tracing::info!("switch2 video GPU: {} shader compiled {}x{} -> {}x{} fit={:?} matrix={} full_range={} pq={} scaler={}",
                if picture.bit_depth == 10 { "P010" } else { "NV12" },
                picture.width, picture.height, self.size.0, self.size.1,
                self.fit, picture.matrix, picture.full_range, picture.pq,
                if picture.bit_depth == 10 { "p010-existing" } else {
                    let scale_x = self.size.0 as f32 / picture.width as f32;
                    let scale_y = self.size.1 as f32 / picture.height as f32;
                    if scale_x >= 0.5 && scale_y >= 0.5 &&
                       (scale_x < 1.0 || scale_y < 1.0) {
                        "nv12-luma-area-chroma-bilinear"
                    } else { "nv12-bilinear" }
                });
        }
        let info = ImageInfo::new((row_bytes as i32, (h*3/2) as i32),
            ColorType::Gray8, AlphaType::Opaque, None);
        // Deep-copy into SkData because the decode thread owns the original NV12.
        // Skia uploads a single-channel texture when the shader draws on Ganesh.
        let image = skia_safe::images::raster_from_data(
            &info, skia_safe::Data::new_copy(&picture.nv12), row_bytes)
            .context("create NV12 Gray8 image")?;
        let plane = image.to_raw_shader(None, SamplingOptions::default(), None)
            .context("make NV12 raw image shader")?;
        let effect = &self.gpu_video_effect.as_ref().unwrap().1;
        self.gpu_video_shader = Some(effect.make_shader(
            skia_safe::Data::new_empty(), &[plane.into()], None)
            .context("bind NV12 shader")?);
        Ok(())
    }

    fn draw_gpu(&mut self, ui: &mut dyn FnMut(&skia_safe::Canvas)) -> bool {
        use skia_safe::gpu;
        if self.stream_enabled && self.current.is_some() && self.gpu_video_shader.is_none() {
            if let Err(e) = self.prepare_video_shader() {
                tracing::warn!("switch2 video GPU: {e:#}; switching to raster fallback");
                return false;
            }
        }
        let queue_family = self.gpu_queue_family;
        let (Some(context), Some(surface)) = (self.gpu_context.as_mut(), self.gpu_surface.as_mut())
            else { return false };
        let start = Instant::now();
        surface.canvas().clear(Color::BLACK); // Letterbox bars and empty stream.
        if self.stream_enabled {
            if let Some(shader) = &self.gpu_video_shader {
                let mut paint = Paint::default();
                paint.set_shader(shader.clone());
                surface.canvas().draw_rect(
                    Rect::from_wh(self.size.0 as f32, self.size.1 as f32), &paint);
            }
        }
        if self.ui_enabled {
            ui(surface.canvas()); // Original Punktfunk frontend and overlay.
        }
        let callback_us = start.elapsed().as_micros();
        // Tell Skia about the EXTERNAL copy's required layout, so its Vulkan
        // bookkeeping remains correct when drawing the next frame as well.
        let state = gpu::vk::mutable_texture_states::new_vulkan(
            gpu::vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            // The same queue family serves both Ganesh and the presenter.
            queue_family,
        );
        context.flush_surface_with_texture_state(surface, &gpu::FlushInfo::default(), Some(&state));
        // Default remains CPU-synchronous until OunceVK real-device testing.
        // A/B opt-in uses same-queue Vulkan submission ordering plus the
        // native ALL_COMMANDS -> TRANSFER_READ barrier (not CPU waits).
        // The native presenter still waits idle after present, preventing
        // an in-flight frame or semaphore from being reused unsafely.
        let async_submit = matches!(option_env!("PF2_ASYNC_GANESH_SUBMIT"), Some("1"));
        let submitted = context.submit(gpu::SubmitInfo {
            sync: if async_submit { gpu::SyncCpu::No } else { gpu::SyncCpu::Yes },
            ..gpu::SubmitInfo::default()
        });
        if !submitted || context.is_device_lost() {
            tracing::error!("switch2 ganesh: Vulkan submit failed or device lost (submitted={submitted})");
            return false;
        }
        if self.frame_number < 3 || (self.frame_number+1) % 120 == 0 {
            tracing::info!("switch2 ganesh draw: frame={} video={} callback_us={} flush_sync_total_us={} layout=TRANSFER_SRC_OPTIMAL",
                self.frame_number + 1, self.stream_enabled, callback_us, start.elapsed().as_micros());
        }
        true
    }

    fn compose(&mut self, ui: &mut dyn FnMut(&skia_safe::Canvas)) -> Result<()> {
        self.prepare_raster()?;
        // Entered only if Ganesh is unavailable. Convert to RGBA lazily so
        // normal video playback never pays the per-pixel CPU conversion cost.
        if let Some(frame) = self.current.as_mut() {
            if frame.rgba.is_empty() {
                let w=frame.width as usize;
                let h=frame.height as usize;
                if frame.bit_depth == 10 {
                    frame.rgba = p010_raster(frame)?;
                } else {
                    anyhow::ensure!(frame.nv12.len() == w*h*3/2, "raster fallback NV12 mismatch");
                    frame.rgba.resize(w*h*4,0);
                    ffi::check("NV12 raster fallback", unsafe { ffi::pf2_nv12_rgba(
                        frame.nv12.as_ptr(),w*h,w,
                        frame.nv12.as_ptr().add(w*h),w*h/2,w,
                        frame.width,frame.height,frame.rgba.as_mut_ptr(),frame.rgba.len(),w*4,
                        frame.matrix,frame.full_range) })?;
                }
            }
        }
        if self.stream_enabled && self.current.is_some() {
            if !self.ui_enabled {
                // With no overlay there is no reason to reserve a separate
                // video frame: keep the scaled image in the present buffer.
                if !self.video_cache_valid {
                    self.pixels.fill(0);
                    for pixel in self.pixels.chunks_exact_mut(4) { pixel[3] = 255; }
                    if let Some(picture) = &self.current {
                        scale_video(picture, self.size, self.fit, &mut self.pixels);
                    }
                    self.video_cache_valid = true;
                }
            } else if self.video_pixels.is_empty() {
                // High-resolution memory-safe fallback: recompose the decoded
                // frame instead of maintaining a 4K cache of the same image.
                self.pixels.fill(0);
                for pixel in self.pixels.chunks_exact_mut(4) { pixel[3] = 255; }
                if let Some(picture) = &self.current {
                    scale_video(picture, self.size, self.fit, &mut self.pixels);
                }
                self.video_cache_valid = false;
            } else {
                if !self.video_cache_valid {
                    self.video_pixels.fill(0);
                    for pixel in self.video_pixels.chunks_exact_mut(4) { pixel[3] = 255; }
                    if let Some(picture) = &self.current {
                        scale_video(picture, self.size, self.fit, &mut self.video_pixels);
                    }
                    self.video_cache_valid = true;
                }
                self.pixels.copy_from_slice(&self.video_pixels);
            }
        } else {
            self.pixels.fill(0);
            for pixel in self.pixels.chunks_exact_mut(4) { pixel[3] = 255; }
            self.video_cache_valid = false;
        }
        if self.ui_enabled {
            let skia_start = Instant::now();
            let canvas = self.surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            let clear_us = skia_start.elapsed().as_micros();
            let callback_start = Instant::now();
            ui(canvas);
            let callback_us = callback_start.elapsed().as_micros();
            let skia_us = skia_start.elapsed().as_micros();
            let pixmap = self.surface.peek_pixels().context("Skia UI pixels")?;
            let source = pixmap.bytes().context("Skia UI raster bytes")?;
            if self.frame_number < 3 || self.frame_number == 120 {
                // 1/64 pixel subsampling keeps this cheap at docked resolution.
                let mut nonempty_alpha = 0usize;
                let mut nonblack_rgb = 0usize;
                let mut total = 0usize;
                for row in source.chunks(pixmap.row_bytes()).take(self.size.1 as usize).step_by(8) {
                    for pixel in row.chunks_exact(4).take(self.size.0 as usize).step_by(8) {
                        total += 1;
                        if pixel[3] != 0 { nonempty_alpha += 1; }
                        if pixel[..3].iter().any(|&c| c != 0) { nonblack_rgb += 1; }
                    }
                }
                tracing::info!("switch2 skia ui: frame={} nonzero-alpha={nonempty_alpha}/{total} nonblack-rgb={nonblack_rgb}/{total} extent={}x{}",
                    self.frame_number + 1, self.size.0, self.size.1);
            }
            let blend_start = Instant::now();
            blend_ui(&mut self.pixels, source, self.size.0 as usize,
                     self.size.1 as usize, pixmap.row_bytes());
            if self.frame_number < 3 || (self.frame_number + 1) % 12 == 0 {
                tracing::info!("switch2 timing skia: frame={} draw_us={} clear_us={} callback_us={} blend_us={}",
                    self.frame_number + 1, skia_us, clear_us, callback_us, blend_start.elapsed().as_micros());
            }
        }
        Ok(())
    }
}

impl Display for NativeDisplay {
    fn size(&self) -> (u32, u32) { self.size }
    fn refresh(&self) -> Refresh { Refresh(self.refresh_hz * 1_000) }
    fn mode(&self) -> OperationMode {
        if self.mode.load(Ordering::Acquire) { OperationMode::Docked } else { OperationMode::Handheld }
    }
    fn request_tv_mode(&mut self, width: u32, height: u32, fps: u32) -> Result<()> {
        tracing::info!(
            "switch2 TV v2 request: {}x{}@{} previous={}x{}@{} docked={} (size is stream policy, not an HDMI override)",
            width, height, fps,
            self.tv_policy.0, self.tv_policy.1, self.tv_policy.2, self.docked,
        );
        if self.tv_policy == (width, height, fps) { return Ok(()); }
        // Moonlight changes the window's rate request while the layer stays alive.
        // Recreating the swapchain is needed only if the actual window extent
        // later changes, not to signal a different display refresh rate.
        ffi::check("OunceVK TV refresh request", unsafe {
            ffi::pf2_presenter_request_tv_mode(self.presenter, width, height, fps)
        })?;
        self.tv_policy = (width, height, fps);
        let (mut actual_hz, mut policy_rc) = (0u32, 0u32);
        let query_rc = unsafe {
            ffi::pf2_presenter_refresh(self.presenter, &mut actual_hz, &mut policy_rc)
        };
        tracing::info!(
            "switch2 TV v2 after rate request: wanted={} Hz query={} Hz query_rc={} policy_rc=0x{:x}; HDMI timing needs TV confirmation",
            fps, actual_hz, query_rc, policy_rc,
        );
        Ok(())
    }
    fn frame(&mut self, ui: &mut dyn FnMut(&skia_safe::Canvas)) -> Result<FrameReport> {
        let mode = self.mode.load(Ordering::Acquire);
        if self.resize_pending {
            if let Err(error) = self.rebuild(mode) {
                tracing::warn!("switch2 swapchain recovery deferred: {error:#}");
            }
            return Ok(FrameReport::default());
        }
        if mode != self.docked {
            // Like Moonlight NX2, require a stable dock reading for 100 ms.
            // Horizon can report the new operation mode before the TV window
            // and its Vulkan surface have settled. Do not destroy Ganesh for
            // an intermediate mode that may disappear on the next poll.
            match self.pending_mode {
                Some((candidate, since)) if candidate == mode
                    && since.elapsed() >= std::time::Duration::from_millis(100) => {
                    if let Err(error) = self.rebuild(mode) {
                        tracing::warn!("switch2 display mode change deferred: {error:#}");
                    }
                }
                Some((candidate, _)) if candidate == mode => {}
                _ => self.pending_mode = Some((mode, Instant::now())),
            }
            // No rendering or stale output-size negotiation during transition.
            return Ok(FrameReport::default());
        } else if self.pending_mode.take().is_some() {
            // A transient reading returned to the current mode: cancel it.
            tracing::info!("switch2 display mode transition cancelled before resize");
        }
        // An accepted 120 Hz request can make Horizon negotiate a new docked
        // output size asynchronously. Observe the refresh independently of
        // window resizing; below we still debounce a changed window extent.
        if self.frame_number % 60 == 0 && self.docked && self.tv_policy.2 == 120 {
            self.update_refresh();
            if self.refresh_hz != 120 && self.frame_number % 300 == 0 {
                tracing::warn!(
                    "switch2 TV v2: 120 Hz requested but VI2 still reports {} Hz; TV/System Settings may not offer 120 Hz",
                    self.refresh_hz,
                );
            }
        }
        if self.frame_number % 15 == 0 {
            let (mut w, mut h, mut policy_rc) = (0, 0, 0);
            let rc = unsafe {
                ffi::pf2_presenter_output_size(self.presenter, &mut w, &mut h, &mut policy_rc)
            };
            // AM can report an intermediate TV size during mode selection.
            // Moonlight verifies settled modes before adopting them. Require
            // a stable value for 500ms instead of a one-frame 4K->1080p bounce.
            if rc == ffi::OK && (w, h) != self.window_size {
                match self.pending_window {
                    Some(((previous_w, previous_h), since)) if (w, h) == (previous_w, previous_h)
                        && since.elapsed() >= std::time::Duration::from_millis(500) => {
                            tracing::info!("switch2 output settled: {}x{} -> {}x{}",
                                self.window_size.0, self.window_size.1, w, h);
                            if let Err(error) = self.rebuild(mode) {
                                tracing::warn!("switch2 settled output resize deferred: {error:#}");
                            }
                            return Ok(FrameReport::default());
                        }
                    Some(((previous_w, previous_h), _)) if (w, h) == (previous_w, previous_h) => {}
                    _ => self.pending_window = Some(((w, h), Instant::now())),
                }
            } else {
                self.pending_window = None;
            }
        }
        // The tracing subscriber is active by the time the first frame arrives.
        // Probe Ganesh once per presenter, but keep the established raster path
        // even when Ganesh is unavailable or unsupported by this driver.
        if !self.ganesh_probe_done && !self.gpu_disabled {
            self.probe_ganesh();
        }
        let frame_start = Instant::now();
        let fresh = self.pull_video()?;
        let mut gpu_presented = false;
        let mut compose_us = 0;
        let mut present_us = 0;
        let mut rc = ffi::OK;
        // Both video NV12 and original UI share the same Ganesh surface.
        // The raster path below remains available on any GPU failure.
        if (self.ui_enabled || self.stream_enabled) && !self.gpu_disabled && self.gpu_surface.is_some() {
            let reused = reuse_unchanged_video(
                self.stream_enabled, self.ui_enabled, fresh, self.gpu_copy_ready,
            );
            if reused || self.draw_gpu(ui) {
                compose_us = frame_start.elapsed().as_micros();
                let present_start = Instant::now();
                if let Some(pulled_at) = self.pulled_at {
                    self.phase.pull_to_submit.note(present_start.duration_since(pulled_at));
                }
                rc = unsafe { ffi::pf2_presenter_frame_gpu(self.presenter) };
                present_us = present_start.elapsed().as_micros();
                if rc == ffi::OK {
                    if let Some(pulled_at) = self.pulled_at {
                        self.phase.pull_to_return.note(pulled_at.elapsed());
                    }
                    gpu_presented = true;
                    self.gpu_copy_ready = true;
                    if reused {
                        self.stream_display_reused = self.stream_display_reused.saturating_add(1);
                    }
                    if self.frame_number == 0 {
                        tracing::info!("switch2 ganesh FIRST_GPU_FRAME: native copy+present rc=0 extent={}x{} (visual confirmation required)",
                            self.size.0, self.size.1);
                    }
                } else {
                    // A swapchain out-of-date is routine during TV mode
                    // negotiation. Keep VkDevice/VI2 alive; resize in-place
                    // after dropping the Ganesh wrappers. Do not immediately
                    // enter the very expensive raster fallback at 4K.
                    if rc == ffi::OUT_OF_DATE {
                        tracing::info!("switch2 Vulkan swapchain out of date: resizing in place");
                        if let Err(error) = self.rebuild(self.docked) {
                            tracing::warn!("switch2 Vulkan resize deferred: {error:#}");
                        }
                        return Ok(FrameReport::default());
                    } else {
                        tracing::warn!("switch2 ganesh present: rc={rc}; using raster fallback");
                        self.gpu_disabled = true;
                        if let Err(error) = self.rebuild(self.docked) {
                            tracing::warn!("switch2 fallback resize deferred: {error:#}");
                        }
                        return Ok(FrameReport::default());
                    }
                }
            } else {
                tracing::warn!("switch2 ganesh frame: draw/submit failed; using raster fallback");
                self.gpu_disabled = true;
                if let Err(error) = self.rebuild(self.docked) {
                    tracing::warn!("switch2 raster recovery deferred: {error:#}");
                }
                self.ganesh_probe_done = true;
                return Ok(FrameReport::default());
            }
        }
        if !gpu_presented {
            self.gpu_copy_ready = false;
            self.compose(ui)?;
            compose_us = frame_start.elapsed().as_micros();
            let present_start = Instant::now();
            let row = self.size.0 as usize * 4;
            if let Some(pulled_at) = self.pulled_at {
                self.phase.pull_to_submit.note(present_start.duration_since(pulled_at));
            }
            rc = unsafe { ffi::pf2_presenter_frame(self.presenter,
                self.pixels.as_ptr(), self.pixels.len(), row) };
            if rc == ffi::OUT_OF_DATE {
                if let Err(error) = self.rebuild(self.docked) {
                    tracing::warn!("switch2 raster swapchain resize deferred: {error:#}");
                }
                return Ok(FrameReport::default());
            }
            ffi::check("OunceVK raster present", rc)?;
            present_us = present_start.elapsed().as_micros();
            if let Some(pulled_at) = self.pulled_at {
                self.phase.pull_to_return.note(pulled_at.elapsed());
            }
        }
        self.last_frame_gpu = gpu_presented;
        self.frame_number = self.frame_number.saturating_add(1);
        if self.stream_enabled {
            self.present_cadence.note(Instant::now(), self.refresh_hz);
        }
        if self.frame_number == 1 || self.frame_number % 30 == 0 {
            self.update_refresh();
        }
        // During streaming the logger flushes to SD synchronously on Horizon.
        // Limit periodic timing events to 1 Hz at 120 Hz, while retaining the
        // first three startup samples and the per-second video pipeline stats.
        if self.frame_number <= 3 || self.frame_number % 120 == 0 {
            tracing::info!("switch2 timing frame: frame={} compose_us={} present_us={} total_us={}",
                self.frame_number, compose_us, present_us, frame_start.elapsed().as_micros());
        }
        if self.stream_enabled {
            self.stream_display_ticks = self.stream_display_ticks.saturating_add(1);
            if fresh { self.stream_display_fresh = self.stream_display_fresh.saturating_add(1); }
            if self.stream_last_stats.elapsed().as_secs() >= 1 {
                let phase = std::mem::take(&mut self.phase);
                let cadence = self.present_cadence.take_samples();
                tracing::info!("switch2 video displayed: ticks={} distinct_frames={} cached_gpu_copies={} renderer={} refresh_hz={} pacing_bounded={} backlog_ticks={} backlog_expired={} max_pending={} phase_publish_pull_avg_us={} phase_publish_pull_max_us={} phase_pull_submit_avg_us={} phase_pull_submit_max_us={} phase_pull_return_avg_us={} phase_pull_return_max_us={} phase_samples={} present_gap_samples={} present_gap_over_1p5x={} present_gap_over_2x={} present_gap_max_us={}",
                    self.stream_display_ticks, self.stream_display_fresh, self.stream_display_reused,
                    if gpu_presented { "ganesh_nv12" } else { "raster" }, self.refresh_hz,
                    self.video.smoothing_enabled.load(Ordering::Relaxed), phase.backlog_ticks, phase.expired, phase.max_pending,
                    phase.publish_to_pull.avg(), phase.publish_to_pull.max_us,
                    phase.pull_to_submit.avg(), phase.pull_to_submit.max_us,
                    phase.pull_to_return.avg(), phase.pull_to_return.max_us,
                    phase.publish_to_pull.samples,
                    cadence.samples, cadence.over_1p5x, cadence.over_2x, cadence.max_us);
                self.stream_display_ticks = 0;
                self.stream_display_fresh = 0;
                self.stream_display_reused = 0;
                self.stream_last_stats = Instant::now();
            }
        }
        if self.frame_number <= 3 || self.frame_number == 120 {
            tracing::info!("switch2 present: frame={} rc={rc} {}x{} video={} ui={} renderer={}",
                self.frame_number, self.size.0, self.size.1,
                self.stream_enabled, self.ui_enabled, if gpu_presented { "ganesh" } else { "raster" });
        }
        if fresh {
            if let Some(picture) = &self.current {
                if picture.capture_pts_ns > 0 {
                    if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                        picture.video_e2e.note(picture.capture_pts_ns, now.as_nanos() as u64);
                    }
                }
            }
        }
        let elapsed = self.current.as_ref()
            .filter(|_| fresh).map(|f| f.decoded_at.elapsed().as_micros().min(u128::from(u32::MAX)) as u32);
        Ok(FrameReport {
            new_picture: fresh,
            decode_to_present_us: elapsed,
            replaced: self.video.replaced.swap(0, Ordering::Relaxed),
            ..Default::default()
        })
    }
    fn set_stream(&mut self, on: bool, fit: Fit) {
        if self.stream_enabled != on || self.fit != fit {
            self.video_cache_valid = false;
            self.gpu_video_shader = None;
            self.gpu_copy_ready = false;
            self.phase = FramePhase::default();
            self.present_cadence = PresentCadence::default();
            self.pulled_at = None;
        }
        self.stream_enabled = on;
        self.fit = fit;
    }
    fn set_ui_layer(&mut self, on: bool) {
        if self.ui_enabled != on {
            // An overlay disappearing must not remain baked into a reused frame.
            self.gpu_copy_ready = false;
            self.video_cache_valid = false;
        }
        self.ui_enabled = on;
    }
    fn read_back(&mut self) -> Result<Option<Rgba>> {
        if self.last_frame_gpu {
            // A captured Ganesh frame must come from the actual Vulkan-backed
            // SkSurface, not the stale CPU fallback buffer. Skia performs the
            // necessary GPU readback only on explicit screenshot requests.
            let Some(surface) = self.gpu_surface.as_mut() else {
                return Err(anyhow!("GPU frame available without Ganesh surface"));
            };
            let row = (self.size.0 as usize).checked_mul(4)
                .context("GPU screenshot row overflow")?;
            let bytes = row.checked_mul(self.size.1 as usize)
                .context("GPU screenshot size overflow")?;
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(bytes).context("GPU screenshot allocation")?;
            pixels.resize(bytes, 0);
            let info = ImageInfo::new((self.size.0 as i32, self.size.1 as i32),
                ColorType::RGBA8888, AlphaType::Unpremul, None);
            if !surface.read_pixels(&info, &mut pixels, row, (0, 0)) {
                return Err(anyhow!("Ganesh Vulkan read_pixels failed ({}x{})", self.size.0, self.size.1));
            }
            return Ok(Some(Rgba { width: self.size.0, height: self.size.1, pixels }));
        }
        if self.pixels.is_empty() { return Ok(None); }
        Ok(Some(Rgba { width: self.size.0, height: self.size.1, pixels: self.pixels.clone() }))
    }
}

impl Drop for NativeDisplay {
    fn drop(&mut self) {
        // Drop texture-backed shader before its Ganesh context and VkDevice.
        self.gpu_video_shader = None;
        self.gpu_video_effect = None;
        drop(self.gpu_surface.take());
        drop(self.gpu_context.take());
        // SAFETY: display exclusively owns the presenter pointer.
        unsafe { ffi::pf2_presenter_close(self.presenter) };
    }
}

/// Scale the newest decoded frame to a pixel buffer, respecting aspect-ratio policy.
/// All image and slice dimensions were validated on construction.
fn scale_video(src: &VideoPicture, dst: (u32, u32), fit: Fit, out: &mut [u8]) {
    let (w, h) = (dst.0 as usize, dst.1 as usize);
    let (sw, sh) = (src.width as usize, src.height as usize);
    if sw == 0 || sh == 0 || src.rgba.len() < sw.saturating_mul(sh).saturating_mul(4) { return; }
    let (sx, sy) = match fit {
        Fit::Stretch => (w as f64 / sw as f64, h as f64 / sh as f64),
        Fit::Contain => {
            let scale = (w as f64 / sw as f64).min(h as f64 / sh as f64);
            (scale, scale)
        }
        Fit::Cover => {
            let scale = (w as f64 / sw as f64).max(h as f64 / sh as f64);
            (scale, scale)
        }
    };
    let ox = (w as f64 - sw as f64 * sx) * 0.5;
    let oy = (h as f64 - sh as f64 * sy) * 0.5;
    for y in 0..h {
        let fy = ((y as f64 - oy) / sy).floor();
        if !(0.0..sh as f64).contains(&fy) { continue; }
        for x in 0..w {
            let fx = ((x as f64 - ox) / sx).floor();
            if !(0.0..sw as f64).contains(&fx) { continue; }
            let from = (fy as usize * sw + fx as usize) * 4;
            let to = (y * w + x) * 4;
            out[to..to + 4].copy_from_slice(&src.rgba[from..from + 4]);
        }
    }
}

/// Skia RGBA8888 is premultiplied; the video beneath is opaque RGBA.
fn blend_ui(dst: &mut [u8], src: &[u8], w: usize, h: usize, stride: usize) {
    for y in 0..h {
        for x in 0..w {
            let d = (y * w + x) * 4;
            let s = y * stride + x * 4;
            let a = u32::from(src[s + 3]);
            for c in 0..3 {
                dst[d + c] = (u32::from(src[s + c]) +
                    u32::from(dst[d + c]) * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod unchanged_video_tests {
    use super::reuse_unchanged_video;

    #[test]
    fn reuse_requires_a_valid_unmodified_gpu_frame() {
        assert!(reuse_unchanged_video(true, false, false, true));
        assert!(!reuse_unchanged_video(true, false, true, true));
        assert!(!reuse_unchanged_video(true, true, false, true));
        assert!(!reuse_unchanged_video(true, false, false, false));
        assert!(!reuse_unchanged_video(false, false, false, true));
    }
}

#[cfg(test)]
mod bounded_handoff_tests {
    use super::PendingPictures;

    #[test]
    fn single_image_is_available_without_waiting() {
        let mut queue = PendingPictures::default();
        assert!(queue.push(1, true).is_none());
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.take_oldest(), Some(1));
        assert_eq!(queue.take_oldest(), None);
    }

    #[test]
    fn one_burst_can_be_presented_in_order() {
        let mut queue = PendingPictures::default();
        assert!(queue.push(1, true).is_none());
        assert!(queue.push(2, true).is_none());
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.take_oldest(), Some(1));
        assert_eq!(queue.take_oldest(), Some(2));
    }

    #[test]
    fn full_burst_discards_oldest_not_newest() {
        let mut queue = PendingPictures::default();
        assert!(queue.push(1, true).is_none());
        assert!(queue.push(2, true).is_none());
        assert_eq!(queue.push(3, true), Some(1));
        assert_eq!(queue.take_oldest(), Some(2));
        assert_eq!(queue.take_oldest(), Some(3));
    }

    #[test]
    fn disabled_path_remains_newest_wins() {
        let mut queue = PendingPictures::default();
        assert!(queue.push(1, false).is_none());
        assert_eq!(queue.push(2, false), Some(1));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.take_oldest(), Some(2));
    }

    #[test]
    fn expired_older_can_be_discarded_without_losing_newest() {
        let mut queue = PendingPictures::default();
        queue.push(1, true);
        queue.push(2, true);
        assert_eq!(queue.oldest(), Some(&1));
        assert_eq!(queue.discard_oldest(), Some(1));
        assert_eq!(queue.take_oldest(), Some(2));
    }
}

#[cfg(test)]
mod pacing_comparison_tests {
    use super::{PresentCadence, select_bounded_video_handoff};
    use std::time::{Duration, Instant};

    #[test]
    fn explicit_override_works_when_stream_requests_lowest_latency() {
        assert!(!select_bounded_video_handoff(None, 0));
        assert!(select_bounded_video_handoff(None, 1));
        assert!(!select_bounded_video_handoff(Some("0"), 3));
        assert!(select_bounded_video_handoff(Some("1"), 0));
        assert!(!select_bounded_video_handoff(Some("other"), 0));
    }

    #[test]
    fn present_cadence_detects_real_long_present_intervals() {
        let start = Instant::now();
        let mut cadence = PresentCadence::default();
        cadence.note(start, 120);
        cadence.note(start + Duration::from_micros(8_333), 120);
        cadence.note(start + Duration::from_micros(25_000), 120);
        assert_eq!(cadence.samples, 2);
        assert_eq!(cadence.over_1p5x, 1);
        assert_eq!(cadence.over_2x, 1);
        assert_eq!(cadence.max_us, 16_667);
        let window = cadence.take_samples();
        assert_eq!(window.samples, 2);
        assert_eq!(cadence.samples, 0);
        assert_eq!(cadence.last_tick, window.last_tick);
    }
}
