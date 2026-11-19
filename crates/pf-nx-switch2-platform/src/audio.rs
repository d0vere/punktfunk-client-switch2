//! Real PCM 48 kHz stereo output through libounce. The official application's
//! audio jitter buffer and PCM Opus decoding remain unchanged.
use std::ptr;
use anyhow::{ensure, Result};
use pf_nx_app::platform::{AudioOut, AudioPort};
use crate::ffi;

pub(crate) struct NativeAudio;
struct Port { handle: *mut ffi::Handle }
// The owner is transferred to the official audio writer's sole thread.
unsafe impl Send for Port {}

const GRAIN: usize = 256;

impl AudioOut for NativeAudio {
    fn open(&self, channels: u8, rate: u32) -> Result<Box<dyn AudioPort>> {
        ensure!(channels == 2 && rate == 48_000,
                "native audout is 48 kHz stereo, requested {rate} Hz and {channels} channels");
        let mut handle = ptr::null_mut();
        // SAFETY: out pointer is valid; C owns the service until Port is dropped.
        ffi::check("audoutInitialize", unsafe { ffi::pf2_audio_open(&mut handle, rate, u32::from(channels)) })?;
        ensure!(!handle.is_null(), "native audio returned null");
        Ok(Box::new(Port { handle }))
    }
}

impl AudioPort for Port {
    fn grain_frames(&self) -> usize { GRAIN }
    fn write(&mut self, pcm: &[f32]) -> Result<()> {
        ensure!(pcm.len() == GRAIN * 2, "PCM grain shape mismatch");
        // SAFETY: Port solely owns the audio handle, PCM slice outlives the blocking call.
        ffi::check("audoutPlayBuffer", unsafe { ffi::pf2_audio_write(self.handle, pcm.as_ptr(), GRAIN) })
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        // SAFETY: the official app stopped the writer before dropping the port.
        unsafe { ffi::pf2_audio_close(self.handle) };
    }
}
