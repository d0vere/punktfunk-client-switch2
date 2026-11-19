//! Switch 2 implementation of the official pf-nx-app::platform seams.
//! Keeps pf-nx-app, Punktfunk QUIC, pairing, settings and core untouched except
//! for the AV1 capability extension. Does NOT import a foreign frontend.
#![cfg(target_os = "horizon")]

use std::cell::{Cell, OnceCell};
use std::time::Instant;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use anyhow::{Result, ensure};
use pf_nx_app::platform::{
    DeviceInfo, Lifecycle, OperationMode, Platform, Storage, SystemEvent,
};

mod audio;
mod display;
mod ffi;
mod input;
mod nv12_pool;
mod video;
// Reuse the official input-to-wire mapping from this exact pinned checkout.
// The old Switch backend consumes rumble/touch conversion helpers as well;
// Switch 2 has its own HID implementations for those two operations.
#[allow(dead_code)]
#[path = "../../pf-nx-platform/src/pad_map.rs"]
mod pad_map;

const DATA_DIR: &str = "sdmc:/switch/punktfunk";
struct Dir(PathBuf);
impl Storage for Dir { fn data_dir(&self) -> &Path { &self.0 } }

struct NativeLifecycle {
    handle: Cell<*mut ffi::Handle>,
    docked: Arc<AtomicBool>,
    closing: bool,
    net_probe_pending: bool,
    // System font mapping is owned by pl:u until userAppExit; one load per title.
    fonts: OnceCell<Vec<&'static [u8]>>,
}

impl NativeLifecycle {
    fn new(docked: Arc<AtomicBool>) -> Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: writable out-pointer; libounce creates and owns BSD socket services.
        ffi::check("socketInitializeDefault / appletLockExit", unsafe { ffi::pf2_runtime_open(&mut handle) })?;
        ensure!(!handle.is_null(), "socket runtime initialized without a handle");
        docked.store(unsafe { ffi::pf2_runtime_docked(handle) } != 0, Ordering::Release);
        Ok(Self { handle: Cell::new(handle), docked, closing: false,
            net_probe_pending: true, fonts: OnceCell::new() })
    }

    fn release(&self) {
        let handle = self.handle.replace(ptr::null_mut());
        if !handle.is_null() {
            // SAFETY: only NativeLifecycle owns this process-wide socket runtime.
            unsafe { ffi::pf2_runtime_close(handle) };
        }
    }
}

// Nonblocking one-shot Rust/SDK comparison, deliberately without TCP connect,
// poll or DNS. A connect_timeout on the main event pump can stall the UI and
// prevent asynchronous logging from draining on this experimental target.
fn probe_rust_network() {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::os::fd::AsRawFd;

    match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        Ok(socket) => {
            let fd = socket.as_raw_fd();
            let local = socket.local_addr();
            match socket.set_nonblocking(true) {
                Ok(()) => tracing::info!(
                    "switch2 network rust: UDP bind+FIONBIO OK fd={fd} local={local:?}"
                ),
                Err(e) => {
                    tracing::warn!(
                        "switch2 network rust: UDP FIONBIO failed fd={fd} local={local:?} error={e} raw={:?}",
                        e.raw_os_error()
                    );
                    // Ask the proven native socket layer to perform the same
                    // FIONBIO on the SAME descriptor. If this succeeds, the
                    // discrepancy is inside the Rust/newlib call ABI or constants.
                    let mut native_rc = -2;
                    let mut native_errno = 0;
                    let mut bsd_result = 0;
                    let bridge_rc = unsafe {
                        ffi::pf2_runtime_nonblock_fd(
                            fd, &mut native_rc, &mut native_errno, &mut bsd_result,
                        )
                    };
                    tracing::warn!(
                        "switch2 network rust-vs-C: bridge={bridge_rc} fd={fd} C_FIONBIO={native_rc} errno={native_errno} bsd=0x{bsd_result:x}"
                    );
                }
            }
        }
        Err(e) => tracing::warn!(
            "switch2 network rust: UDP bind failed error={e} raw={:?}",
            e.raw_os_error()
        ),
    }
}

impl Lifecycle for NativeLifecycle {
    fn hide_splash(&self) {}

    // The official Switch 1 client asks pl:u for system faces. On Switch 2
    // the previous port used Lifecycle's empty default, leaving Skia with zero
    // system faces and no guaranteed text font. libounce exposes the same pl:u
    // ABI and pf-nx-sys already owns the validated font loader and exit hook.
    fn fallback_fonts(&self) -> &[&'static [u8]] {
        self.fonts.get_or_init(|| {
            let begin = Instant::now();
            let faces = pf_nx_sys::pl::shared_fonts("en");
            let bytes: usize = faces.iter().map(|f| f.len()).sum();
            tracing::info!(
                "switch2 fonts: {} shared faces, {} bytes, loaded in {} ms",
                faces.len(), bytes, begin.elapsed().as_millis()
            );
            faces
        })
    }


    fn poll_events(&mut self, out: &mut Vec<SystemEvent>) {
        if self.closing { self.exit(); }
        if self.net_probe_pending {
            self.net_probe_pending = false;
            let mut probe = ffi::NetProbe::default();
            // SAFETY: the BSD runtime is still alive; this creates, binds and closes
            // a diagnostic UDP socket without sending traffic.
            let rc = unsafe { ffi::pf2_runtime_net_probe(self.handle.get(), &mut probe) };
            if rc != ffi::OK {
                tracing::warn!("switch2 network probe: native diagnostic failed rc={rc}");
            } else {
                tracing::info!(
                    "switch2 network probe: C socket={} errno={} bsd=0x{:x}; bind={} errno={} bsd=0x{:x}; nonblocking={} errno={} bsd=0x{:x}",
                    probe.socket_rc, probe.socket_errno, probe.socket_bsd_result,
                    probe.bind_rc, probe.bind_errno, probe.bind_bsd_result,
                    probe.nonblock_rc, probe.nonblock_errno, probe.nonblock_bsd_result
                );
            }
            let mut buffers = ffi::SocketTuning::default();
            // SAFETY: the runtime is alive; samples a temporary UDP socket.
            let rc = unsafe { ffi::pf2_runtime_socket_tuning(self.handle.get(), &mut buffers) };
            if rc == ffi::OK {
                tracing::info!(
                    "switch2 BSD socket buffers: boosted={} boost_rc=0x{:x} udp_config_tx_kb={} udp_config_rx_kb={} fresh_socket_send_kb={} fresh_socket_recv_kb={}",
                    buffers.boosted, buffers.boost_result,
                    buffers.requested_tx / 1024, buffers.requested_rx / 1024,
                    if buffers.socket_sndbuf < 0 { -1 } else { buffers.socket_sndbuf / 1024 },
                    if buffers.socket_rcvbuf < 0 { -1 } else { buffers.socket_rcvbuf / 1024 },
                );
            } else {
                tracing::warn!("switch2 BSD socket buffers: diagnostic failed rc={rc}");
            }
            probe_rust_network();
        }
        let mut flags = 0;
        let mut docked = 0;
        // SAFETY: unique owner; polling on the app's main thread.
        let rc = unsafe { ffi::pf2_runtime_poll(self.handle.get(), &mut flags, &mut docked) };
        if rc != ffi::OK {
            tracing::error!("Switch 2 applet pump failed: {rc}");
            out.push(SystemEvent::Suspend);
            self.closing = true;
            return;
        }
        let new_mode = docked != 0;
        let old_mode = self.docked.swap(new_mode, Ordering::AcqRel);
        if flags & 1 != 0 { out.push(SystemEvent::FocusLost); }
        if flags & 2 != 0 { out.push(SystemEvent::FocusGained); }
        if flags & 4 != 0 {
            out.push(SystemEvent::Suspend);
            // The first suspension may be just HOME, not exit. Only exit
            // requests should close the title; C flag 32 marks a real close.
        }
        if flags & 8 != 0 { out.push(SystemEvent::Resume); }
        if flags & 16 != 0 || new_mode != old_mode {
            out.push(SystemEvent::OperationMode(if new_mode {
                OperationMode::Docked
            } else { OperationMode::Handheld }));
        }
        if flags & 32 != 0 { self.closing = true; }
    }

    fn exit(&self) -> ! {
        // The official app must first drain the session; socketExit happens
        // only in exit(), not when HOME or a normal stream closes.
        self.release();
        std::process::exit(0)
    }
}

impl Drop for NativeLifecycle { fn drop(&mut self) { self.release(); } }

// A marker outside the app data directory survives the failure that would
// otherwise leave a zero-byte app log on the second run.
fn boot_stage(stage: &'static [u8]) {
    debug_assert_eq!(stage.last(), Some(&0));
    // SAFETY: every call site passes a static, NUL-terminated ASCII label.
    unsafe { ffi::pf2_runtime_boot_trace(stage.as_ptr().cast()) };
}

pub fn platform() -> Result<Platform> {
    boot_stage(b"platform.begin\0");
    // Force extraction of the C archive member defining the weak Skia/ICU
    // mmap/munmap/pread compatibility symbols before the final static link.
    // SAFETY: this function has no arguments and no state; it is a link anchor.
    unsafe { ffi::pf2_skia_compat_link_guard() };
    let mode = Arc::new(AtomicBool::new(false));
    let lifecycle = match NativeLifecycle::new(mode.clone()) {
        Ok(lifecycle) => lifecycle,
        Err(e) => { boot_stage(b"platform.socket.failed\0"); return Err(e); }
    };
    boot_stage(b"platform.socket.ok\0");
    // Avoid recursive Rust mkdir on Horizon when the folder already exists.
    // The native backend treats EEXIST as successful and prepares logs, too.
    if let Err(e) = ffi::check("prepare Punktfunk data directories", unsafe {
        ffi::pf2_runtime_prepare_storage()
    }) {
        boot_stage(b"platform.storage.failed\0");
        return Err(e);
    }
    boot_stage(b"platform.storage.ok\0");
    let video_slot = Arc::new(display::VideoSlot::default());
    let display = match display::NativeDisplay::new(video_slot.clone(), mode) {
        Ok(display) => display,
        Err(e) => { boot_stage(b"platform.display.failed\0"); return Err(e); }
    };
    boot_stage(b"platform.display.ok\0");
    let pads = match input::NativePads::new() {
        Ok(pads) => pads,
        Err(e) => { boot_stage(b"platform.input.failed\0"); return Err(e); }
    };
    boot_stage(b"platform.input.ok\0");
    // SAFETY: runtime still alive. appletGetAppletType is a pure query.
    let applet_mode = unsafe { ffi::pf2_runtime_applet(lifecycle.handle.get()) } != 0;
    boot_stage(b"platform.ready\0");
    Ok(Platform {
        display: Box::new(display),
        video: Arc::new(video::NativeVideo { frames: video_slot }),
        audio: Arc::new(audio::NativeAudio),
        pads: Box::new(pads),
        storage: Arc::new(Dir(PathBuf::from(DATA_DIR))),
        lifecycle: Box::new(lifecycle),
        log: Some(Box::new(std::io::stderr())),
        device: DeviceInfo {
            platform: "switch", model: "Nintendo Switch 2".to_owned(),
            firmware: String::new(), gpu: "OunceVK native Vulkan".to_owned(),
            ui: "official Skia raster / RGBA composition / Vulkan scanout".to_owned(),
            applet_mode, languages: Vec::new(),
        },
    })
}
