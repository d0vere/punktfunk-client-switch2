//! Original libounce Switch 2 HID sample source, using the official
//! pf-nx-platform pad-map verbatim via relative module import.
use std::ptr;
use std::time::{Duration, Instant};
use anyhow::Result;
use pf_nx_app::platform::{Motion, PadFeedback, PadKind, PadSample, Pads, TouchPoint};
use crate::{ffi, pad_map};

// Guard against a rumble stop that is lost while streaming or reconnecting.
// The host normally renews active feedback ~120 ms apart; 750 ms also allows
// legacy hosts with ~500 ms renewals. A normal explicit stop remains immediate.
const RUMBLE_FEEDBACK_TIMEOUT: Duration = Duration::from_millis(750);

pub(crate) struct NativePads {
    handle: *mut ffi::Handle,
    touch_seen: bool,
    handheld_attached: bool,
    motion_seen: [bool; 4],
    rumble_seen: [bool; 4],
    rumble_live_until: [Option<Instant>; 4],
    rumble_on_commands: [u32; 4],
    rumble_off_commands: [u32; 4],
    rumble_log_at: Instant,
    last_style: [u32; 4],
}
// Input is owned and called by the application's input thread, not shared.
unsafe impl Send for NativePads {}

impl NativePads {
    pub(crate) fn new() -> Result<Self> {
        let mut handle = ptr::null_mut();
        // SAFETY: valid writable out-pointer.
        ffi::check("padConfigureInput", unsafe { ffi::pf2_input_open(&mut handle) })?;
        anyhow::ensure!(!handle.is_null(), "libounce returned a null HID object");
        Ok(Self { handle, touch_seen: false, handheld_attached: false, motion_seen: [false; 4],
            rumble_seen: [false; 4], rumble_live_until: [None; 4],
            rumble_on_commands: [0; 4], rumble_off_commands: [0; 4],
            rumble_log_at: Instant::now(), last_style: [0; 4] })
    }
}

// Stable FFI style tags shared with Pf2PadRaw in pf2_native.h.
fn kind_from_style(style: u32) -> PadKind {
    match style {
        1 | 2 => PadKind::JoyConPair,
        3 => PadKind::JoyConLeft,
        4 => PadKind::JoyConRight,
        5 => PadKind::SwitchPro,
        _ => PadKind::Other,
    }
}

impl Pads for NativePads {
    fn poll(&mut self, out: &mut Vec<PadSample>) {
        out.clear();
        let mut samples = [ffi::PadRaw::default(); 4];
        // SAFETY: raw samples is exactly four repr(C) structs; exclusively owned handle.
        let rc = unsafe { ffi::pf2_input_poll(self.handle, samples.as_mut_ptr()) };
        if rc != ffi::OK {
            tracing::warn!("Switch 2 HID poll failed: {rc}");
            return;
        }
        // If feedback ceases without a stop, never leave a level-triggered
        // HID vibration latched indefinitely. The SDK calls run on the same
        // input thread as the original feedback, with no extra worker/timer.
        let now = Instant::now();
        for slot in 0..4 {
            if self.rumble_live_until[slot].is_some_and(|deadline| now >= deadline) {
                let rc = unsafe { ffi::pf2_input_rumble(self.handle, slot as u32, 0, 0) };
                if rc == ffi::OK || rc == ffi::NOT_READY {
                    self.rumble_live_until[slot] = None;
                    tracing::warn!(slot, "Switch 2 rumble safety stop: no recent feedback");
                } else {
                    // Retry a failed stop without hammering HID every 4 ms.
                    self.rumble_live_until[slot] = Some(now + Duration::from_millis(50));
                }
            }
        }
        if now.duration_since(self.rumble_log_at) >= Duration::from_secs(10) {
            if self.rumble_on_commands.iter().any(|&count| count != 0) ||
               self.rumble_off_commands.iter().any(|&count| count != 0) {
                tracing::info!(nonzero = ?self.rumble_on_commands,
                    zero = ?self.rumble_off_commands,
                    "Switch 2 rumble feedback (10s)");
            }
            self.rumble_on_commands = [0; 4];
            self.rumble_off_commands = [0; 4];
            self.rumble_log_at = now;
        }
        // This flag is independent of the Pro/Joy-Con type of slot 0.
        let handheld = unsafe { ffi::pf2_input_handheld_attached(self.handle) } != 0;
        if handheld != self.handheld_attached {
            self.handheld_attached = handheld;
            tracing::info!(attached = handheld,
                "Switch 2 handheld Joy-Cons joined player 1");
        }
        for raw in samples {
            let slot = raw.slot as usize;
            if slot >= self.last_style.len() {
                tracing::warn!(slot, "ignoring invalid native HID slot");
                continue;
            }
            if raw.connected == 0 {
                self.last_style[slot] = 0;
                self.motion_seen[slot] = false;
                self.rumble_seen[slot] = false;
                self.rumble_live_until[slot] = None;
                continue;
            }
            if self.last_style[slot] != raw.style {
                self.last_style[slot] = raw.style;
                self.motion_seen[slot] = false;
                self.rumble_seen[slot] = false;
                tracing::info!(slot, style = raw.style, kind = ?kind_from_style(raw.style),
                    "Switch 2 physical controller identified");
            }
            let (lt, rt) = pad_map::triggers(raw.buttons);
            out.push(PadSample {
                slot: raw.slot as u8,
                kind: kind_from_style(raw.style),
                buttons: pad_map::buttons(raw.buttons),
                lx: pad_map::stick(raw.left_x),
                ly: pad_map::stick(raw.left_y),
                rx: pad_map::stick(raw.right_x),
                ry: pad_map::stick(raw.right_y),
                lt, rt,
            });
        }
    }

    fn touches(&mut self, out: &mut Vec<TouchPoint>) {
        out.clear();
        let mut raw = [ffi::TouchRaw::default(); 16];
        let mut count = 0u32;
        // SAFETY: the array and count have the same fixed layout as pf2_native.h.
        let rc = unsafe { ffi::pf2_input_touches(self.handle, raw.as_mut_ptr(), &mut count) };
        if rc != ffi::OK {
            tracing::warn!("Switch 2 touch poll failed: {rc}");
            return;
        }
        // As in Moonlight NX2, panel coordinates are always 1280x720.
        // A release contact must not create a fresh TouchDown on the host.
        for point in &raw[..count.min(16) as usize] {
            if point.attributes & 2 != 0 { continue; }
            out.push(TouchPoint {
                id: point.finger_id,
                x: ((point.x as f32) / 1280.0).clamp(0.0, 1.0),
                y: ((point.y as f32) / 720.0).clamp(0.0, 1.0),
            });
        }
        if !out.is_empty() && !self.touch_seen {
            self.touch_seen = true;
            tracing::info!("Switch 2 touch HID active: first contact received");
        }
    }

    fn motion(&mut self, slot: u8) -> Option<Motion> {
        if usize::from(slot) >= self.motion_seen.len() { return None; }
        let mut raw = ffi::MotionRaw::default();
        // SAFETY: writable C-layout output, valid native HID handle; slot checked in C.
        if unsafe { ffi::pf2_input_motion(self.handle, u32::from(slot), &mut raw) } != ffi::OK {
            return None;
        }
        if !self.motion_seen[usize::from(slot)] {
            self.motion_seen[usize::from(slot)] = true;
            tracing::info!("Switch 2 pad {slot}: six-axis HID active");
        }
        let (gyro, accel) = pad_map::motion(raw.accel, raw.gyro);
        if !gyro.iter().chain(&accel).all(|v| v.is_finite()) { return None; }
        Some(Motion { gyro, accel })
    }

    fn feedback(&mut self, slot: u8, feedback: &PadFeedback) {
        if usize::from(slot) >= self.rumble_seen.len() {
            tracing::warn!(slot, "ignoring rumble for invalid controller slot");
            return;
        }
        let PadFeedback::Rumble { low, high } = *feedback;
        let index = usize::from(slot);
        if low != 0 || high != 0 {
            self.rumble_on_commands[index] = self.rumble_on_commands[index].saturating_add(1);
        } else {
            self.rumble_off_commands[index] = self.rumble_off_commands[index].saturating_add(1);
        }
        // SAFETY: controlled input pointer, valid slot checked in C.
        let rc = unsafe { ffi::pf2_input_rumble(self.handle, u32::from(slot), low, high) };
        if rc == ffi::OK {
            self.rumble_live_until[index] = if low != 0 || high != 0 {
                Some(Instant::now() + RUMBLE_FEEDBACK_TIMEOUT)
            } else {
                None
            };
        } else if rc == ffi::NOT_READY {
            self.rumble_live_until[index] = None;
        } else {
            tracing::debug!("HD rumble unavailable for slot {slot}: {rc}");
        }
        if rc == ffi::OK && (low != 0 || high != 0)
            && !self.rumble_seen[usize::from(slot)] {
            self.rumble_seen[usize::from(slot)] = true;
            tracing::info!("Switch 2 pad {slot}: vibration sent to HID device");
        }
    }

    fn release_all(&mut self) {
        self.rumble_live_until = [None; 4];
        // SAFETY: exclusive ownership of this HID device.
        unsafe { ffi::pf2_input_release_all(self.handle) };
    }
}

impl Drop for NativePads {
    fn drop(&mut self) {
        // SAFETY: unique owning handle. This also stops motors before freeing.
        unsafe { ffi::pf2_input_close(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_styles_match_the_physical_nintendo_device() {
        assert_eq!(kind_from_style(1), PadKind::JoyConPair); // handheld
        assert_eq!(kind_from_style(2), PadKind::JoyConPair); // two detached Joy-Cons
        assert_eq!(kind_from_style(3), PadKind::JoyConLeft);
        assert_eq!(kind_from_style(4), PadKind::JoyConRight);
        assert_eq!(kind_from_style(5), PadKind::SwitchPro);
        assert_eq!(kind_from_style(0), PadKind::Other);
    }
}
