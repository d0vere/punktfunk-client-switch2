//! Switch touchscreen routing: native passthrough, trackpad and absolute pointer.
//! Reuses Punktfunk's pure desktop gesture engine; the console has the same wire protocol.

use std::time::Instant;

use pf_client_core::trust::TouchMode;
use punktfunk_core::input::scroll::{ScrollEvent, ScrollPhase, ScrollSource};
use punktfunk_core::input::{InputEvent, InputKind};

// This upstream module only uses std; reusing it preserves the original clients'
// acceleration, tap/drag, two-finger scrolling and right-click behavior.
// This Switch-only import uses Gestures, not the desktop-only SteamTouchMouse
// helper or its display-density setter. Keep upstream code untouched.
#[allow(dead_code)]
#[path = "../../../../vendor/punktfunk/crates/client/pf-presenter/src/touch.rs"]
mod gesture_model;
use gesture_model::{Abs, Act, Gestures, TouchScrollPhase};
use punktfunk_core::video_fit::{self, VideoFit};

use crate::platform::TouchPoint;

/// Where the picture sits: the output it is drawn in and the stream frame, both in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub output: (u32, u32),
    pub frame: (u32, u32),
}

impl Screen {
    /// A finger, across the screen, in frame pixels. Off the picture it lands on its edge.
    fn to_frame(self, x: f32, y: f32) -> (i32, i32) {
        let place = video_fit::place(VideoFit::Fit, self.output, self.frame);
        let (fx, fy) = place.to_frame(
            f64::from(x) * f64::from(self.output.0),
            f64::from(y) * f64::from(self.output.1),
        );
        let last = |n: u32| f64::from(n.saturating_sub(1));
        (
            fx.round().min(last(self.frame.0)) as i32,
            fy.round().min(last(self.frame.1)) as i32,
        )
    }

    /// The frame size as the touch events carry it.
    fn flags(self) -> u32 {
        ((self.frame.0 & 0xffff) << 16) | (self.frame.1 & 0xffff)
    }
}

/// A finger the host holds: the console's id, the host's slot, where it was last sent.
struct Held {
    id: u32,
    slot: u32,
    at: (i32, i32),
}

/// Tracks either native host contacts or gestures translated to host mouse events.
/// The original Touch mode remains the default for existing Switch installations.
pub struct TouchRouter {
    held: Vec<Held>,
    mode: TouchMode,
    gestures: Gestures,
    previous: Vec<TouchPoint>,
    started: Instant,
    mouse_down: [bool; 4],
}

impl Default for TouchRouter {
    fn default() -> Self { Self::new(TouchMode::Touch) }
}

impl TouchRouter {
    pub fn new(mode: TouchMode) -> Self {
        Self {
            held: Vec::new(), mode,
            gestures: Gestures::new(mode != TouchMode::Pointer),
            previous: Vec::new(), started: Instant::now(), mouse_down: [false; 4],
        }
    }

    pub fn set_mode(&mut self, mode: TouchMode, wire: &mut Vec<InputEvent>) {
        if self.mode == mode { return; }
        self.release_all(wire);
        self.mode = mode;
        self.gestures = Gestures::new(mode != TouchMode::Pointer);
    }

    /// 250 Hz input tick: native contacts in Touch mode, Punktfunk gestures otherwise.
    pub fn step(&mut self, fingers: &[TouchPoint], screen: Screen, wire: &mut Vec<InputEvent>) {
        match self.mode {
            TouchMode::Touch => self.direct_step(fingers, screen, wire),
            TouchMode::Off => {},
            TouchMode::Trackpad | TouchMode::Pointer => self.gesture_step(fingers, screen, wire),
        }
    }

    fn direct_step(&mut self, fingers: &[TouchPoint], screen: Screen, wire: &mut Vec<InputEvent>) {
        self.held.retain(|h| {
            let stays = fingers.iter().any(|f| f.id == h.id);
            if !stays {
                wire.push(up(h.slot));
            }
            stays
        });
        if screen.frame.0 == 0 || screen.frame.1 == 0 {
            return;
        }
        for f in fingers {
            let at = screen.to_frame(f.x, f.y);
            if let Some(h) = self.held.iter_mut().find(|h| h.id == f.id) {
                if h.at != at {
                    h.at = at;
                    wire.push(event(InputKind::TouchMove, h.slot, at, screen.flags()));
                }
                continue;
            }
            let slot = (0..).find(|s| self.held.iter().all(|h| h.slot != *s)).unwrap_or(0);
            self.held.push(Held { id: f.id, slot, at });
            wire.push(event(InputKind::TouchDown, slot, at, screen.flags()));
        }
    }

    fn gesture_step(&mut self, fingers: &[TouchPoint], screen: Screen, wire: &mut Vec<InputEvent>) {
        if screen.frame.0 == 0 || screen.frame.1 == 0 || screen.output.0 == 0 || screen.output.1 == 0 {
            self.release_all(wire);
            return;
        }
        let t = self.started.elapsed().as_secs_f64() * 1000.0;
        let mut acts = Vec::new();
        // HID is a snapshot, unlike SDL. Translate missing IDs to ups before the new downs.
        for old in &self.previous {
            if !fingers.iter().any(|f| f.id == old.id) {
                acts.extend(self.gestures.up(u64::from(old.id), t));
            }
        }
        for finger in fingers {
            let wx = finger.x.clamp(0.0, 1.0) * screen.output.0 as f32;
            let wy = finger.y.clamp(0.0, 1.0) * screen.output.1 as f32;
            let (x, y) = screen.to_frame(finger.x, finger.y);
            let abs = Abs { x, y, w: screen.frame.0, h: screen.frame.1 };
            if let Some(old) = self.previous.iter().find(|p| p.id == finger.id) {
                if (old.x, old.y) != (finger.x, finger.y) {
                    acts.extend(self.gestures.motion(u64::from(finger.id), wx, wy, abs, t));
                }
            } else {
                acts.extend(self.gestures.down(u64::from(finger.id), wx, wy, abs, t));
            }
        }
        acts.extend(self.gestures.tick(t));
        self.previous.clear();
        self.previous.extend_from_slice(fingers);
        self.forward(acts, wire);
    }

    fn forward(&mut self, acts: Vec<Act>, wire: &mut Vec<InputEvent>) {
        for act in acts {
            let event = match act {
                Act::MoveRel { dx, dy } => Some(event(InputKind::MouseMove, 0, (dx, dy), 0)),
                Act::MoveAbs(a) => Some(event(InputKind::MouseMoveAbs, 0, (a.x, a.y),
                    ((a.w & 0xffff) << 16) | (a.h & 0xffff))),
                Act::Button { gs, down } => {
                    if let Some(held) = self.mouse_down.get_mut(gs as usize) {
                        *held = down;
                    }
                    Some(event(if down { InputKind::MouseButtonDown } else { InputKind::MouseButtonUp }, gs, (0, 0), 0))
                }
                Act::Scroll { axis, delta, phase } => {
                    let phase = match phase {
                        TouchScrollPhase::Begin => ScrollPhase::Begin,
                        TouchScrollPhase::Update => ScrollPhase::Update,
                        TouchScrollPhase::End => ScrollPhase::End,
                        TouchScrollPhase::Cancel => ScrollPhase::Cancel,
                    };
                    // The shared desktop gesture model's wheel orientation is reversed
                    // on the Switch trackpad; invert both axes, preserving Begin/End.
                    let delta = if self.mode == TouchMode::Trackpad {
                        delta.saturating_neg()
                    } else {
                        delta
                    };
                    let ev = ScrollEvent { source: ScrollSource::Touch, phase, axis,
                        delta, axis_units: false }.to_event();
                    Some(ev)
                }
                // A two-finger twist is interpreted as a dial by the desktop client.
                // The Switch's dial remains gamepad-driven; do not send those acts as mouse input.
                Act::CycleStats | Act::Dial { .. } | Act::DialCommit | Act::DialCancel => None,
            };
            if let Some(ev) = event { wire.push(ev); }
        }
    }

    /// End all host contacts and mouse holds on mode change, dial open and stream exit.
    pub fn release_all(&mut self, wire: &mut Vec<InputEvent>) {
        wire.extend(self.held.drain(..).map(|h| up(h.slot)));
        self.previous.clear();
        let acts = self.gestures.reset();
        self.forward(acts, wire);
        for (gs, held) in self.mouse_down.iter_mut().enumerate() {
            if *held {
                wire.push(event(InputKind::MouseButtonUp, gs as u32, (0, 0), 0));
                *held = false;
            }
        }
    }
}

fn event(kind: InputKind, slot: u32, (x, y): (i32, i32), flags: u32) -> InputEvent {
    InputEvent {
        kind,
        _pad: [0; 3],
        code: slot,
        x,
        y,
        flags,
    }
}

fn up(slot: u32) -> InputEvent {
    event(InputKind::TouchUp, slot, (0, 0), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDHELD: Screen = Screen {
        output: (1280, 720),
        frame: (1280, 720),
    };

    fn finger(id: u32, x: f32, y: f32) -> TouchPoint {
        TouchPoint { id, x, y }
    }

    fn kinds(wire: &[InputEvent]) -> Vec<(InputKind, u32, i32, i32)> {
        wire.iter().map(|e| (e.kind, e.code, e.x, e.y)).collect()
    }

    #[test]
    fn a_tap_goes_down_and_up_on_the_picture() {
        let mut r = TouchRouter::default();
        let mut wire = Vec::new();
        r.step(&[finger(7, 0.5, 0.25)], HANDHELD, &mut wire);
        r.step(&[finger(7, 0.5, 0.25)], HANDHELD, &mut wire);
        r.step(&[], HANDHELD, &mut wire);
        assert_eq!(
            kinds(&wire),
            [(InputKind::TouchDown, 0, 640, 180), (InputKind::TouchUp, 0, 0, 0)]
        );
        assert_eq!(wire[0].flags, (1280 << 16) | 720);
    }

    #[test]
    fn a_second_finger_takes_the_next_slot_and_slots_are_reused() {
        let mut r = TouchRouter::default();
        let mut wire = Vec::new();
        r.step(&[finger(3, 0.0, 0.0), finger(9, 1.0, 1.0)], HANDHELD, &mut wire);
        r.step(&[finger(9, 0.5, 0.5)], HANDHELD, &mut wire);
        r.step(&[finger(9, 0.5, 0.5), finger(4, 0.25, 0.5)], HANDHELD, &mut wire);
        assert_eq!(
            kinds(&wire),
            [
                (InputKind::TouchDown, 0, 0, 0),
                (InputKind::TouchDown, 1, 1279, 719),
                (InputKind::TouchUp, 0, 0, 0),
                (InputKind::TouchMove, 1, 640, 360),
                (InputKind::TouchDown, 0, 320, 360),
            ]
        );
    }

    #[test]
    fn a_smaller_frame_scales_and_bars_clamp() {
        let mut r = TouchRouter::default();
        let mut wire = Vec::new();
        // A 4:3 frame on the 16:9 screen: bars left and right, the middle is the frame's.
        let screen = Screen {
            output: (1280, 720),
            frame: (960, 720),
        };
        r.step(&[finger(1, 0.5, 0.5), finger(2, 0.0, 0.5)], screen, &mut wire);
        assert_eq!(
            kinds(&wire),
            [(InputKind::TouchDown, 0, 480, 360), (InputKind::TouchDown, 1, 0, 360)]
        );
    }

    #[test]
    fn release_lifts_everything() {
        let mut r = TouchRouter::default();
        let mut wire = Vec::new();
        r.step(&[finger(1, 0.5, 0.5), finger(2, 0.1, 0.1)], HANDHELD, &mut wire);
        wire.clear();
        r.release_all(&mut wire);
        assert_eq!(
            kinds(&wire),
            [(InputKind::TouchUp, 0, 0, 0), (InputKind::TouchUp, 1, 0, 0)]
        );
        wire.clear();
        r.release_all(&mut wire);
        assert!(wire.is_empty());
    }

    #[test]
    fn trackpad_moves_relative_without_teleporting_on_down() {
        let mut r = TouchRouter::new(TouchMode::Trackpad);
        let mut wire = Vec::new();
        r.step(&[finger(1, 0.25, 0.25)], HANDHELD, &mut wire);
        assert!(wire.is_empty());
        r.step(&[finger(1, 0.30, 0.25)], HANDHELD, &mut wire);
        assert!(wire.iter().any(|e| e.kind == InputKind::MouseMove && e.x > 0));
        assert!(!wire.iter().any(|e| e.kind == InputKind::MouseMoveAbs));
    }

    #[test]
    fn trackpad_tap_clicks_and_mode_change_releases_drag() {
        let mut r = TouchRouter::new(TouchMode::Trackpad);
        let mut wire = Vec::new();
        r.step(&[finger(1, 0.3, 0.3)], HANDHELD, &mut wire);
        r.step(&[], HANDHELD, &mut wire);
        assert!(wire.iter().any(|e| e.kind == InputKind::MouseButtonDown && e.code == 1));
        assert!(wire.iter().any(|e| e.kind == InputKind::MouseButtonUp && e.code == 1));
        wire.clear();
        r.set_mode(TouchMode::Touch, &mut wire);
        r.step(&[finger(2, 0.3, 0.3)], HANDHELD, &mut wire);
        assert!(wire.iter().any(|e| e.kind == InputKind::TouchDown));
        assert!(!wire.iter().any(|e| e.kind == InputKind::MouseMove));
    }


    #[test]
    fn trackpad_two_finger_scroll_uses_native_scroll_protocol() {
        let mut r = TouchRouter::new(TouchMode::Trackpad);
        let mut wire = Vec::new();
        r.step(&[finger(1, 0.3, 0.3)], HANDHELD, &mut wire);
        r.step(&[finger(1, 0.3, 0.3), finger(2, 0.4, 0.3)], HANDHELD, &mut wire);
        wire.clear();
        r.step(&[finger(1, 0.3, 0.45), finger(2, 0.4, 0.45)], HANDHELD, &mut wire);
        let vertical = wire.iter().find_map(ScrollEvent::from_event)
            .filter(|e| e.axis == 0).expect("vertical two-finger scroll");
        // Both fingers moved down: host scroll must move down, not up.
        assert!(vertical.delta > 0, "trackpad vertical scroll reversed: {}", vertical.delta);
        assert!(!wire.iter().any(|e| e.kind == InputKind::TouchMove));
    }

}
