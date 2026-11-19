//! Non-blocking streaming keyboard, drawn in the existing Skia overlay.
//!
//! A three-contact tap toggles it. Modifiers are real held VK events, not
//! committed text: Alt then Tab/F4 sends the chord the Windows host expects.
//! Keyboard ownership stays on the 250-Hz input thread; the render thread only
//! reads its state under a short mutex.

use std::time::{Duration, Instant};

use pf_console_ui::theme::{Fonts, W};
use punktfunk_core::input::{key_vk, InputEvent, InputKind};
use skia_safe::{Canvas, Color4f, Paint, RRect, Rect};

use crate::platform::TouchPoint;
use super::pads::key_event;

const TAP_MAX: Duration = Duration::from_millis(430);
const MAX_TRAVEL: f32 = 0.045; // normalized screen coordinates

// The row definitions are the drawing, hit-testing, and event authority.
// Keep Tab and F4 visible together for Alt+Tab and Alt+F4.
const ROWS: &[&[&str]] = &[
    &["Esc", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "Close"],
    &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "Backspace"],
    &["Tab", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"],
    &["A", "S", "D", "F", "G", "H", "J", "K", "L", "Enter"],
    &["Shift", "Z", "X", "C", "V", "B", "N", "M", ".", "/", "Shift"],
    &["Ctrl", "Win", "Alt", "Space", "Alt", "Left", "Down", "Up", "Right"],
];
const MODS: &[(&str, u8)] = &[("Shift", 0x10), ("Ctrl", 0x11), ("Alt", 0x12), ("Win", 0x5B)];

#[derive(Clone, Copy)]
struct Contact {
    id: u32,
    x: f32,
    y: f32,
    at: Instant,
    label: &'static str,
}

/// The overlay is always hidden initially, including after a session reconnect.
pub(super) struct StreamKeyboard {
    shown: bool,
    held: [bool; 4],
    /// While a three-finger gesture is in progress, do not leak touch/mouse to the host.
    three: Option<(Instant, Vec<(u32, f32, f32)>, bool)>,
    suppress_until_empty: bool,
    single: Option<Contact>,
    text_capable: bool,
    keyboard_granted: bool,
}

impl StreamKeyboard {
    pub(super) fn new(text_capable: bool, keyboard_granted: bool) -> Self {
        Self {
            shown: false, held: [false; 4], three: None,
            suppress_until_empty: false, single: None,
            text_capable, keyboard_granted,
        }
    }

    pub(super) fn shown(&self) -> bool { self.shown }

    pub(super) fn toggle(&mut self, wire: &mut Vec<InputEvent>) {
        if self.shown {
            self.close(wire);
        } else if self.keyboard_granted {
            self.shown = true;
            tracing::info!("stream on-screen keyboard: opened");
        } else {
            tracing::warn!("stream on-screen keyboard: host denied keyboard input");
        }
    }

    pub(super) fn close(&mut self, wire: &mut Vec<InputEvent>) {
        let was_open = self.shown;
        self.shown = false;
        self.single = None;
        // A sticky modifier must never leak into the host after the keyboard closes.
        for (held, (_, vk)) in self.held.iter_mut().zip(MODS) {
            if std::mem::take(held) {
                wire.push(key_event(InputKind::KeyUp, *vk));
            }
        }
        if was_open { tracing::info!("stream on-screen keyboard: closed; modifiers released"); }
    }

    /// Touchscreen snapshots -> keyboard or a three-finger toggle. Returns `true` while
    /// any contact belongs to the keyboard/gesture; the ordinary TouchRouter then
    /// releases its own host contacts and sends nothing else for this tick.
    pub(super) fn touch_step(&mut self, fingers: &[TouchPoint], wire: &mut Vec<InputEvent>) -> bool {
        // Three fingers are recognized by their *actual* IDs, not by sequential HID slots.
        if fingers.len() >= 3 && self.three.is_none() && !self.suppress_until_empty {
            self.three = Some((Instant::now(), fingers.iter().take(3)
                .map(|f| (f.id, f.x, f.y)).collect(), false));
            self.suppress_until_empty = true;
            self.single = None;
        }
        if let Some((since, starts, moved)) = &mut self.three {
            if since.elapsed() > TAP_MAX { *moved = true; }
            for f in fingers {
                if let Some((_, x, y)) = starts.iter().find(|(id, _, _)| *id == f.id) {
                    if (f.x - x).abs() > MAX_TRAVEL || (f.y - y).abs() > MAX_TRAVEL {
                        *moved = true;
                    }
                }
            }
            if fingers.is_empty() {
                let good = !*moved && since.elapsed() <= TAP_MAX;
                self.three = None;
                self.suppress_until_empty = false;
                if good { self.toggle(wire); }
            }
            return true;
        }
        if self.suppress_until_empty {
            if fingers.is_empty() { self.suppress_until_empty = false; }
            return true;
        }
        if !self.shown { return false; }

        // A key is struck on finger release only. Motion and multi-touch on the
        // keyboard are swallowed; a drag never types a different key on release.
        match (self.single, fingers) {
            (None, [f]) => {
                if let Some(label) = hit(f.x, f.y) {
                    self.single = Some(Contact {
                        id: f.id, x: f.x, y: f.y, at: Instant::now(), label,
                    });
                }
            }
            (Some(start), []) => {
                self.single = None;
                if start.at.elapsed() <= TAP_MAX {
                    self.strike(start.label, wire);
                }
            }
            (Some(start), [f]) if start.id == f.id => {
                if (f.x - start.x).abs() > MAX_TRAVEL ||
                    (f.y - start.y).abs() > MAX_TRAVEL ||
                    hit(f.x, f.y) != Some(start.label) {
                    self.single = None;
                }
            }
            _ => self.single = None,
        }
        true
    }

    fn strike(&mut self, label: &'static str, wire: &mut Vec<InputEvent>) {
        if label == "Close" { self.close(wire); return; }
        if let Some((i, (_, vk))) = MODS.iter().enumerate().find(|(_, (name, _))| *name == label) {
            self.held[i] = !self.held[i];
            wire.push(key_event(if self.held[i] { InputKind::KeyDown } else { InputKind::KeyUp }, *vk));
            return;
        }
        // Printable characters without held modifiers use the layout-independent
        // Unicode path; keys in a chord use VK edges instead.
        if label.len() == 1 && self.held.iter().all(|v| !v) && self.text_capable {
            wire.push(InputEvent {
                kind: InputKind::TextInput, _pad: [0; 3],
                code: u32::from(label.chars().next().unwrap().to_ascii_lowercase()),
                x: 0, y: 0, flags: 0,
            });
            return;
        }
        let name = match label {
            "Backspace" => "backspace", "Enter" => "enter", "Space" => "space",
            "Left" => "left", "Right" => "right", "Down" => "down", "Up" => "up",
            "Esc" => "esc", "Tab" => "tab", other => other,
        };
        let special_vk = match label { "." => Some(0xBE), "/" => Some(0xBF), _ => None };
        if let Some(vk) = special_vk.or_else(|| key_vk(name)) {
            wire.push(key_event(InputKind::KeyDown, vk));
            wire.push(key_event(InputKind::KeyUp, vk));
        } else if self.text_capable && self.held.iter().all(|v| !v) {
            if let Some(ch) = label.chars().next() {
                wire.push(InputEvent {
                    kind: InputKind::TextInput, _pad: [0; 3], code: u32::from(ch),
                    x: 0, y: 0, flags: 0,
                });
            }
        }
    }

    pub(super) fn draw(&self, canvas: &Canvas, fonts: &Fonts, width: u32, height: u32) {
        if !self.shown { return; }
        let w = width as f32;
        let h = height as f32;
        let top = h * 0.47;
        let pane = Rect::from_xywh(0.0, top, w, h - top);
        fill(canvas, pane, Color4f::new(0.06, 0.075, 0.10, 0.94), 10.0);
        let legend = "ON-SCREEN KEYBOARD   •   modifiers stay held until tapped again   •   three-finger tap to close";
        let font_size = f64::from((h / 720.0 * 13.0).clamp(11.0, 26.0));
        fonts.draw(canvas, legend, f64::from(w * 0.03), f64::from(top + h * 0.032),
            W::Medium, font_size, Color4f::new(0.9, 0.92, 0.96, 1.0));
        for (r, label) in layout() {
            let active = MODS.iter().enumerate().any(|(i, (name, _))| *name == label && self.held[i]);
            fill(canvas, Rect::from_ltrb(r.left * w, r.top * h, r.right * w, r.bottom * h),
                if active { Color4f::new(0.25, 0.48, 0.84, 1.0) }
                else { Color4f::new(0.19, 0.22, 0.29, 1.0) }, 5.0);
            let fsize = (h / 720.0 * 18.0).clamp(12.0, 38.0) as f64;
            let tw = fonts.measure(label, W::Medium, fsize);
            fonts.draw(canvas, label,
                f64::from((r.left + r.right) * 0.5 * w - tw * 0.5),
                f64::from((r.top * 0.44 + r.bottom * 0.56) * h),
                W::Medium, fsize, Color4f::new(1.0, 1.0, 1.0, 1.0));
        }
    }
}

fn fill(canvas: &Canvas, rect: Rect, color: Color4f, radius: f32) {
    let mut paint = Paint::new(color, None);
    paint.set_anti_alias(true);
    canvas.draw_rrect(RRect::new_rect_xy(rect, radius, radius), &paint);
}

/// Key hit rectangles in *normalized output space*. Draw and HID hit-test use
/// identical coordinates regardless of handheld scale or Ganesh UI resolution.
fn layout() -> Vec<(Rect, &'static str)> {
    let mut out = Vec::new();
    let row_start = 0.53;
    let row_h = 0.071;
    let row_step = 0.077;
    for (row_index, row) in ROWS.iter().enumerate() {
        let total: f32 = row.iter().map(|label| match *label {
            "Space" => 4.0, "Backspace" | "Shift" | "Enter" | "Close" | "Ctrl" | "Win" | "Alt" | "Tab" => 1.6,
            _ => 1.0,
        }).sum();
        let mut x = 0.019;
        let usable = 0.962;
        for label in *row {
            let weight = match *label {
                "Space" => 4.0, "Backspace" | "Shift" | "Enter" | "Close" | "Ctrl" | "Win" | "Alt" | "Tab" => 1.6,
                _ => 1.0,
            };
            let dw = usable * weight / total;
            let y = row_start + row_index as f32 * row_step;
            out.push((Rect::from_ltrb(x + 0.002, y, x + dw - 0.002, y + row_h), *label));
            x += dw;
        }
    }
    out
}

fn hit(x: f32, y: f32) -> Option<&'static str> {
    layout().into_iter().find_map(|(r, label)|
        (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).then_some(label))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(id: u32, x: f32, y: f32) -> TouchPoint { TouchPoint { id, x, y } }

    #[test]
    fn three_finger_tap_toggles_once_per_gesture() {
        let mut k = StreamKeyboard::new(true, true);
        let mut wire = Vec::new();
        let three = [f(7, 0.2, 0.3), f(9, 0.4, 0.3), f(11, 0.6, 0.3)];
        assert!(k.touch_step(&three, &mut wire));
        assert!(k.touch_step(&[], &mut wire));
        assert!(k.shown());
        assert!(k.touch_step(&[], &mut wire));
        assert!(k.shown());
        assert!(k.touch_step(&three, &mut wire));
        assert!(k.touch_step(&[], &mut wire));
        assert!(!k.shown());
    }

    #[test]
    fn sticky_alt_sends_exact_alt_tab_then_alt_f4_chord() {
        let mut k = StreamKeyboard::new(true, true);
        let mut wire = Vec::new();
        k.shown = true;
        k.strike("Alt", &mut wire);
        k.strike("Tab", &mut wire);
        k.strike("F4", &mut wire);
        assert_eq!(wire.iter().map(|e| (e.kind, e.code)).collect::<Vec<_>>(), [
            (InputKind::KeyDown, 0x12),
            (InputKind::KeyDown, 0x09), (InputKind::KeyUp, 0x09),
            (InputKind::KeyDown, 0x73), (InputKind::KeyUp, 0x73),
        ]);
        k.close(&mut wire);
        assert_eq!((wire.last().unwrap().kind, wire.last().unwrap().code), (InputKind::KeyUp, 0x12));
    }

    #[test]
    fn no_keyboard_grant_never_opens_or_sends_keydown() {
        let mut k = StreamKeyboard::new(true, false);
        let mut wire = Vec::new();
        k.toggle(&mut wire);
        assert!(!k.shown());
        assert!(wire.is_empty());
    }
}
