//! FFI with ports/switch2/native/pf2_native.h; all pointer ownership is wrapped
//! by RAII types in sibling modules. This module intentionally has no assumptions
//! about libnx v1 struct sizes: pointers are opaque, data layouts are fixed-width.
use core::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pitch_y: u32,
    pub pitch_uv: u32,
    pub pts_ns: u64,
    pub plane_y: *const u8,
    pub plane_uv: *const u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DecoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: u32,
    pub image_count: u32,
    pub bit_depth: u32, // fills the old alignment padding; sizeof remains 48
    pub codec_data: *const u8,
    pub codec_data_length: usize,
    pub max_sample_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PadRaw {
    pub slot: u32,
    pub connected: u32,
    pub style: u32,
    pub buttons: u64,
    pub left_x: i32,
    pub left_y: i32,
    pub right_x: i32,
    pub right_y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TouchRaw {
    pub finger_id: u32,
    pub attributes: u32,
    pub x: u32,
    pub y: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MotionRaw {
    pub accel: [f32; 3],
    pub gyro: [f32; 3],
}

pub type Handle = c_void;

#[repr(C)]
#[derive(Default)]
pub struct SocketTuning {
    pub boosted: u32,
    pub boost_result: u32,
    pub requested_tx: u32,
    pub requested_rx: u32,
    pub socket_sndbuf: i32,
    pub socket_rcvbuf: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NetProbe {
    pub socket_rc: i32,
    pub socket_errno: i32,
    pub socket_bsd_result: u32,
    pub bind_rc: i32,
    pub bind_errno: i32,
    pub bind_bsd_result: u32,
    pub nonblock_rc: i32,
    pub nonblock_errno: i32,
    pub nonblock_bsd_result: u32,
}

// Raw Vulkan handles are borrowed from the native presenter; they must never
// outlive it. On the 64-bit Switch 2 target these four handles are pointers.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkBorrowedContext {
    pub instance: *mut c_void,
    pub physical_device: *mut c_void,
    pub device: *mut c_void,
    pub queue: *mut c_void,
    pub queue_family: u32,
    pub physical_api_version: u32,
}

/// Borrowed VkImage; underlying VkImage and VkDeviceMemory belong to Pf2Presenter.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VkOffscreenImage {
    pub image: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub queue_family: u32,
}

pub const OK: i32 = 0;
pub const NOT_READY: i32 = -4;
pub const OUT_OF_DATE: i32 = -6;

#[link(name = "pf2native", kind = "static")]
unsafe extern "C" {
    pub fn pf2_decoder_open(out: *mut *mut Handle, s: *const DecoderSettings) -> i32;
    pub fn pf2_decoder_submit(d: *mut Handle, data: *const u8, len: usize, pts_ns: i64, timeout_ns: u64) -> i32;
    pub fn pf2_decoder_acquire(d: *mut Handle, frame: *mut Frame, timeout_ns: u64) -> i32;
    pub fn pf2_decoder_acquire_into(d: *mut Handle, frame: *mut Frame,
        packed: *mut u8, capacity: usize, timeout_ns: u64) -> i32;
    pub fn pf2_decoder_release(d: *mut Handle) -> i32;
    pub fn pf2_decoder_flush(d: *mut Handle) -> i32;
    pub fn pf2_decoder_close(d: *mut Handle);
    pub fn pf2_decoder_last_sdk_result(d: *const Handle) -> u32;
    pub fn pf2_decoder_last_sdk_stage(d: *const Handle) -> u32;
    pub fn pf2_decoder_last_copy_us(d: *const Handle) -> u32;
    pub fn pf2_nv12_rgba(y: *const u8, y_len: usize, y_stride: usize,
        uv: *const u8, uv_len: usize, uv_stride: usize, width: u32, height: u32,
        rgba: *mut u8, rgba_len: usize, rgba_stride: usize, matrix: i32, full_range: i32) -> i32;
    pub fn pf2_presenter_open(out: *mut *mut Handle, width: u32, height: u32) -> i32;
    pub fn pf2_presenter_open_fixed(out: *mut *mut Handle, width: u32, height: u32) -> i32;
    pub fn pf2_presenter_size(p: *const Handle, width: *mut u32, height: *mut u32) -> i32;
    pub fn pf2_presenter_resize(p: *mut Handle) -> i32;
    pub fn pf2_presenter_request_tv_mode(p: *mut Handle, width: u32, height: u32, fps: u32) -> i32;
    pub fn pf2_presenter_output_size(p: *const Handle, width: *mut u32,
        height: *mut u32, policy_rc: *mut u32) -> i32;
    pub fn pf2_presenter_refresh(p: *const Handle, hz: *mut u32, policy_rc: *mut u32) -> i32;
    pub fn pf2_presenter_vk_borrow(p: *const Handle, out: *mut VkBorrowedContext) -> i32;
    pub fn pf2_presenter_hdr10_surface(p: *const Handle, supported: *mut u32) -> i32;
    pub fn pf2_vk_instance_proc(instance: *mut c_void, name: *const core::ffi::c_char) -> *const c_void;
    pub fn pf2_vk_device_proc(device: *mut c_void, name: *const core::ffi::c_char) -> *const c_void;
    pub fn pf2_presenter_frame(p: *mut Handle, rgba: *const u8, bytes: usize, stride: usize) -> i32;
    pub fn pf2_presenter_gpu_image(p: *mut Handle, out: *mut VkOffscreenImage) -> i32;
    pub fn pf2_presenter_frame_gpu(p: *mut Handle) -> i32;
    pub fn pf2_presenter_close(p: *mut Handle);
    pub fn pf2_audio_open(out: *mut *mut Handle, rate: u32, channels: u32) -> i32;
    pub fn pf2_audio_write(a: *mut Handle, pcm: *const f32, frames: usize) -> i32;
    pub fn pf2_audio_close(a: *mut Handle);
    pub fn pf2_input_open(out: *mut *mut Handle) -> i32;
    pub fn pf2_input_motion(input: *mut Handle, slot: u32, out: *mut MotionRaw) -> i32;
    pub fn pf2_input_poll(input: *mut Handle, samples: *mut PadRaw) -> i32;
    pub fn pf2_input_handheld_attached(input: *const Handle) -> u32;
    pub fn pf2_input_touches(input: *mut Handle, samples: *mut TouchRaw, count: *mut u32) -> i32;
    pub fn pf2_input_close(input: *mut Handle);
    pub fn pf2_input_rumble(input: *mut Handle, slot: u32, low: u16, high: u16) -> i32;
    pub fn pf2_input_release_all(input: *mut Handle);
    pub fn pf2_skia_compat_link_guard();
    pub fn pf2_runtime_boot_trace(stage: *const core::ffi::c_char);
    pub fn pf2_runtime_prepare_storage() -> i32;
    pub fn pf2_runtime_open(out: *mut *mut Handle) -> i32;
    pub fn pf2_runtime_socket_tuning(r: *const Handle, out: *mut SocketTuning) -> i32;
    pub fn pf2_runtime_net_probe(r: *const Handle, out: *mut NetProbe) -> i32;
    pub fn pf2_runtime_nonblock_fd(fd: i32, ioctl_rc: *mut i32,
        ioctl_errno: *mut i32, bsd_result: *mut u32) -> i32;
    pub fn pf2_runtime_poll(r: *mut Handle, flags: *mut u32, docked: *mut u32) -> i32;
    pub fn pf2_runtime_docked(r: *const Handle) -> u32;
    pub fn pf2_runtime_applet(r: *const Handle) -> u32;
    pub fn pf2_runtime_close(r: *mut Handle);
}

pub fn check(what: &str, rc: i32) -> anyhow::Result<()> {
    if rc == OK { Ok(()) } else { anyhow::bail!("{what}: libounce bridge returned {rc}") }
}

const _: [(); 24] = [(); std::mem::size_of::<SocketTuning>()];
const _: [(); 36] = [(); std::mem::size_of::<NetProbe>()];
const _: [(); 48] = [(); std::mem::size_of::<DecoderSettings>()];
const _: [(); 40] = [(); std::mem::size_of::<Frame>()];
const _: [(); 40] = [(); std::mem::size_of::<PadRaw>()];
const _: [(); 16] = [(); std::mem::size_of::<TouchRaw>()];

#[cfg(target_pointer_width = "64")]
const _: [(); 40] = [(); std::mem::size_of::<VkBorrowedContext>()];
#[cfg(target_pointer_width = "64")]
const _: [(); 24] = [(); std::mem::size_of::<VkOffscreenImage>()];
