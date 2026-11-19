//! The shared console as this title's menu. Hands a launch or a quit back as a [`UiOutcome`],
//! reloading the settings document on every entry.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pf_client_core::console::{OverlayAction, PointerButton, PointerInput, SessionPhase};
use pf_client_core::menu_nav::{MenuEvent, PadInfo};
use pf_console_ui::{Console, ConsoleEntry, ConsoleHandles, ConsoleOptions, InputSource, Platform, Viewport};
use punktfunk_core::config::{GamepadPref, Mode};

use super::connect::{self, ConnectOutcome, Launch, Started};
use super::pads::{resolve_kind, MenuPads};
use super::{testrun, App, UiOutcome, SUSPENDED};
use crate::core::model::GamepadType;
use crate::core::perf::{ArtSnapshot, Perf};
use crate::core::settings::NxSettings;
use crate::platform::{PadKind, PadSample, TouchPoint};
use crate::services::store::console::ConsoleStore;
use crate::services::store::{self, StateWriter};
use crate::ui::Service;

/// One-finger pointer bridge for the original Punktfunk console UI. Second fingers
/// never generate mouse presses. Screen coordinates are native output pixels.
#[derive(Default)]
struct MenuTouch {
    held: Option<(u32, f32, f32)>,
}

impl MenuTouch {
    fn step(&mut self, fingers: &[TouchPoint], size: (u32, u32), events: &mut Vec<PointerInput>) {
        if size.0 == 0 || size.1 == 0 {
            if self.held.take().is_some() {
                events.push(PointerInput::Cancel);
            }
            return;
        }
        let at = |f: &TouchPoint| {
            ((f.x.clamp(0.0, 1.0) * size.0 as f32),
             (f.y.clamp(0.0, 1.0) * size.1 as f32))
        };
        if let Some((id, x, y)) = self.held {
            if let Some(f) = fingers.iter().find(|f| f.id == id) {
                let (nx, ny) = at(f);
                if (x, y) != (nx, ny) {
                    events.push(PointerInput::Move { x: nx, y: ny });
                    self.held = Some((id, nx, ny));
                }
                return;
            }
            events.push(PointerInput::Up { x, y, button: PointerButton::Primary });
            self.held = None;
        }
        if let Some(f) = fingers.first() {
            let (x, y) = at(f);
            self.held = Some((f.id, x, y));
            events.push(PointerInput::Down { x, y, button: PointerButton::Primary, touch: true });
        }
    }
}

/// The console's output as a stream mode, for a "Native" pick.
pub(super) fn native_mode(app: &App) -> Mode {
    let (width, height) = app.display.size();
    Mode {
        width,
        height,
        refresh_hz: app.display.refresh().hz(),
    }
}

/// Native-window policy for the existing Resolution and Refresh rate settings.
/// Auto and Match window restore the system's docked setting. An explicit
/// standard 16:9 size requests that native-window extent; it is not a guarantee
/// that the HDMI timing itself changes. Above 1440p the TV stays at 60 Hz.
pub(super) fn tv_request(settings: &store::Settings) -> (u32, u32, u32) {
    if connect::follows_output(settings) {
        return (0, 0, 60);
    }
    let (w, h) = (settings.width, settings.height);
    if !matches!((w, h), (1280, 720) | (1920, 1080) | (2560, 1440) | (3840, 2160)) {
        return (0, 0, 60);
    }
    let fps = if settings.refresh_hz >= 120 && h <= 1440 { 120 } else { 60 };
    (w, h, fps)
}

/// Runs the console until it commits a launch or asks to leave.
pub(super) fn run(app: &mut App, notice: Option<String>) -> Result<UiOutcome> {
    let state = store::load();
    let writer = Arc::new(StateWriter::spawn(state.clone()));
    let store = Arc::new(ConsoleStore::new(state, writer));
    let handles = ConsoleHandles::new();
    let mut service = Service::new(handles.clone(), store.clone(), app.identity.clone());
    let caps = crate::core::caps::video_caps();
    let opts = ConsoleOptions {
        device_name: "Nintendo Switch".into(),
        device_code: pf_client_core::trust::device_code(&app.identity),
        // The player reads the version; the pins stay in the log and the bundle.
        version: Some(crate::core::VERSION.into()),
        system_keyboard: false,
        tv: true,
        fallback_ui: false,
        pyrowave_ok: caps.pyrowave,
        av1_ok: caps.av1,
        // The console reports no headroom figure for its display.
        hdr_no_headroom: false,
        store: Some(store.clone()),
        platform: Platform::Switch,
        gpu_cache_bytes: pf_console_ui::DEFAULT_GPU_CACHE_BYTES,
        screen: None,
        profiles: false,
        // No profile picker, so a locked profile is dialled as it stands.
        prompts_unlock: false,
        // Hide for me is a profile's own list, and the profile picker is off here.
        hides: false,
        spatial: false,
        languages: app.device.languages.clone(),
    };
    let scripted = app.test_run.as_ref().and_then(|run| {
        let host = run.host.as_deref()?;
        if let (None, Some(pin)) = (service.row_for(host, run.port), &run.pin) {
            let port = run.port.unwrap_or(pf_client_core::deeplink::DEFAULT_PORT);
            if let Err(e) = service.pair_now(host, port, pin, &opts.device_name) {
                tracing::warn!("test run: pairing with {host}:{port} failed: {e}");
            }
        }
        if let Some(mgmt) = run.mgmt_port {
            service.set_mgmt_port(host, run.port, mgmt);
        }
        let row = service.row_for(host, run.port);
        if row.is_none() {
            tracing::warn!("test run: {host} is not a paired host here");
        }
        // The menu scenario names a host to pair with or reach, not to stream from.
        row.filter(|_| run.scenario == testrun::Scenario::Stream)
    });
    let entry = match scripted {
        Some(row) => ConsoleEntry::Stream(Box::new(row)),
        None => service
            .selected_row()
            .map_or(ConsoleEntry::Home, |row| ConsoleEntry::Library(Box::new(row))),
    };
    let built = Instant::now();
    let faces = app.lifecycle.fallback_fonts();
    let mut console = Console::new(opts, entry, &handles)
        .context("console shell")?
        .with_fallback_fonts(faces)
        .context("system fonts")?;
    tracing::info!(
        "console: built with {} system faces in {} ms",
        faces.len(),
        built.elapsed().as_millis()
    );
    let ui_viewport = |app: &App| {
        let (w, h) = app.display.ui_size();
        Viewport::plain(w, h)
    };
    let mut viewport = ui_viewport(app);
    if app.device.platform == "switch" && app.device.model == "Nintendo Switch 2" {
        tracing::info!("console: skip synchronous all-tabs warm-up on Switch 2; render menu now");
    } else {
        let warm = Instant::now();
        app.display.frame(&mut |canvas| {
            // Keep the original behavior on Switch 1 and on the dev box.
            console.warm_up(canvas, &viewport, |_| {});
        })?;
        tracing::info!("console: warmed the tabs in {} ms", warm.elapsed().as_millis());
    }
    if let Some(notice) = notice {
        handles.console.set_notice(notice);
    }
    app.display.set_stream(false, crate::platform::Fit::Contain);

    let mut perf = Perf::new();
    let mut menu_pads = MenuPads::default();
    let mut batch: Vec<PadSample> = Vec::new();
    let mut menu_touch = MenuTouch::default();
    let mut fingers: Vec<TouchPoint> = Vec::new();
    let mut pointer_events: Vec<PointerInput> = Vec::new();
    let mut infos: Vec<PadInfo> = Vec::new();
    let mut menu_out: Vec<MenuEvent> = Vec::new();
    let mut connect: Option<Started> = None;
    let mut frames: u64 = 0;
    let mut native = native_mode(app);
    // Remember the applied selection, not the menu's displayed row: Store.save
    // updates the shared settings object as soon as the player changes a value.
    let mut tv_applied = None;
    let outcome = 'ui: loop {
        let frame_start = Instant::now();
        app.system_events();
        // Read after the frame that resized the display, not at the dock event a frame earlier.
        if native_mode(app) != native {
            viewport = ui_viewport(app);
            native = native_mode(app);
        }

        batch.clear();
        fingers.clear();
        if let Some(pads) = app.pads.as_mut() {
            pads.poll(&mut batch);
            pads.touches(&mut fingers);
        }
        pointer_events.clear();
        menu_touch.step(&fingers, app.display.ui_size(), &mut pointer_events);
        for ev in pointer_events.drain(..) {
            console.pointer(ev);
        }
        if let Some(scripted) = app.test_run.as_ref().and_then(testrun::TestRun::pad_sample) {
            batch.insert(0, scripted);
        }
        menu_out.clear();
        if let Some(state) = menu_pads.poll(&batch, service.pad_test, Instant::now(), &mut menu_out) {
            handles.console.set_pad_test(state);
        }
        for ev in menu_out.drain(..) {
            console.menu(ev, InputSource::Pad);
        }

        service.tick();
        let requested_tv = store.with(|state| tv_request(&state.settings));
        if tv_applied != Some(requested_tv) {
            tracing::info!(
                "switch2 TV v2 menu selection: requested={}x{}@{} (0x0=system output); docked={}",
                requested_tv.0, requested_tv.1, requested_tv.2,
                matches!(app.display.mode(), crate::platform::OperationMode::Docked),
            );
            match app.display.request_tv_mode(requested_tv.0, requested_tv.1, requested_tv.2) {
                Ok(()) => tv_applied = Some(requested_tv),
                Err(error) => tracing::warn!("switch2 TV mode request rejected: {error:#}"),
            }
        }

        while let Some(action) = console.take_action() {
            match action {
                OverlayAction::Launch {
                    addr,
                    port,
                    fp_hex,
                    launch,
                    title,
                    preset,
                    profile,
                    request_access: _,
                } => {
                    if SUSPENDED.load(std::sync::atomic::Ordering::Relaxed) {
                        continue;
                    }
                    let want = Launch {
                        addr,
                        port,
                        fp_hex,
                        launch,
                        preset,
                        profile,
                    };
                    let what = format!("{title} on {}:{}", want.addr, want.port);
                    let overlay = app.test_run.as_ref().and_then(|run| run.settings.as_ref());
                    let pad = batch.first().map(|s| s.kind);
                    match connect::start(&store, &app.media, &app.identity, want, native, overlay, pad) {
                        Ok(started) => {
                            tracing::info!("console: launching {what}");
                            console.session_phase(SessionPhase::Connecting);
                            connect = Some(started);
                        }
                        Err(e) => {
                            tracing::warn!("console: launch refused: {e:#}");
                            handles
                                .console
                                .set_notice(pf_l10n::client::dial_stream_start_failed().to_string());
                        }
                    }
                }
                OverlayAction::CancelConnect => {
                    if connect.take().is_some() {
                        tracing::info!("console: connect cancelled");
                        console.session_phase(SessionPhase::Ended(None));
                    }
                }
                OverlayAction::Quit => {
                    tracing::info!("console: quit");
                    break 'ui UiOutcome::Quit(connect::exit_plan(&service, &app.identity));
                }
                // No clipboard, no launcher files, and the stream shows by leaving this loop.
                OverlayAction::ShowStream
                | OverlayAction::CopyText(_)
                | OverlayAction::PinShortcut { .. }
                | OverlayAction::SaveLauncherFile { .. } => {}
            }
        }

        if connect.as_ref().is_some_and(|(h, ..)| h.is_finished()) {
            let (handle, target, settings) = connect.take().expect("just checked");
            let connected = match handle.join() {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("the connect thread panicked")),
            };
            break 'ui UiOutcome::Launch(Box::new(ConnectOutcome {
                connected,
                target,
                settings,
                exit_plan: connect::exit_plan(&service, &app.identity),
            }));
        }

        // VI2 may switch between 60 and 120 Hz after the first present.
        let period = Duration::from_nanos(app.display.refresh().period_ns());
        let idled = if console.idle() {
            std::thread::sleep(period);
            period
        } else {
            Duration::ZERO
        };
        let stored_kind = store.with(|s| s.settings.gamepad_type());
        let label = pad_label(&batch);
        let pad_pref = label.map(|_| legend_pref(shown_kind(stored_kind, &batch).to_core()));
        infos.clear();
        infos.extend(pad_infos(&batch, stored_kind));
        // The cost excludes the vblank wait: `frame` blocks on the swapchain.
        let before_draw = frame_start.elapsed().saturating_sub(idled);
        let mut drew = Duration::ZERO;
        app.display.frame(&mut |canvas| {
            let t = Instant::now();
            console.frame(canvas, &viewport, label, pad_pref, &infos);
            drew = t.elapsed();
        })?;
        frames += 1;
        if let Some(report) = perf.frame(before_draw + drew, art_snapshot()) {
            tracing::info!("{}{}", report.line(), app.clocks_suffix());
        }
        if let Some(done) = testrun::menu_step(app, frames) {
            break 'ui done;
        }
    };

    if let Some(report) = perf.finish(art_snapshot()) {
        tracing::info!("{}", report.line());
    }
    service.stop();
    Ok(outcome)
}

/// The pad chip's label: the first pad's kind, or `None` with no pad in hand.
fn pad_label(batch: &[PadSample]) -> Option<&'static str> {
    batch.first().map(|s| match s.kind {
        PadKind::JoyConPair => "Joy-Con",
        PadKind::JoyConLeft => "Joy-Con (L)",
        PadKind::JoyConRight => "Joy-Con (R)",
        PadKind::SwitchPro => "Pro Controller",
        PadKind::Other => pf_l10n::console::console_pad_fallback_name(),
    })
}

/// The kind the shell draws: the stored pick, or the pad itself on Automatic.
fn shown_kind(stored: GamepadType, batch: &[PadSample]) -> GamepadType {
    batch.first().map_or(stored, |s| resolve_kind(stored, s.kind))
}

/// The legend's pad family. The shell's Nintendo legend puts confirm on south; here A confirms
/// on east, so the letters legend is the one that names the engraved buttons.
// TODO(core): `Platform::Switch` still draws the Nintendo legend's confirm on south.
fn legend_pref(pref: GamepadPref) -> GamepadPref {
    match pf_console_ui::glyphs::GlyphStyle::from_pref(Some(pref)) {
        pf_console_ui::glyphs::GlyphStyle::Nintendo => GamepadPref::XboxOne,
        _ => pref,
    }
}

fn pad_infos(batch: &[PadSample], stored: GamepadType) -> impl Iterator<Item = PadInfo> + '_ {
    batch.iter().map(move |s| PadInfo {
        name: pad_label(std::slice::from_ref(s))
            .unwrap_or_else(pf_l10n::console::console_pad_fallback_name)
            .into(),
        key: format!("slot{}", s.slot),
        pref: shown_kind(stored, std::slice::from_ref(s)).to_core(),
        steam_virtual: false,
        battery: None,
        detail: pf_l10n::console::console_pad_player(s.slot + 1),
        forwarded: true,
        rumble: true,
        audio: false,
    })
}

fn art_snapshot() -> ArtSnapshot {
    let a = pf_console_ui::art_stats();
    ArtSnapshot {
        decoded: a.decoded,
        total_us: a.total_us,
        max_us: a.max_us,
        native_scaled: a.native_scaled,
    }
}

#[cfg(test)]
mod menu_touch_tests {
    use super::*;

    fn f(id: u32, x: f32, y: f32) -> TouchPoint {
        TouchPoint { id, x, y }
    }

    #[test]
    fn taps_move_and_release_in_surface_pixels() {
        let mut touch = MenuTouch::default();
        let mut events = Vec::new();
        touch.step(&[f(7, 0.5, 0.25)], (1920, 1080), &mut events);
        touch.step(&[f(7, 0.75, 0.25)], (1920, 1080), &mut events);
        touch.step(&[], (1920, 1080), &mut events);
        assert_eq!(events, [
            PointerInput::Down { x: 960.0, y: 270.0, button: PointerButton::Primary, touch: true },
            PointerInput::Move { x: 1440.0, y: 270.0 },
            PointerInput::Up { x: 1440.0, y: 270.0, button: PointerButton::Primary },
        ]);
    }

    #[test]
    fn a_second_finger_cannot_click_while_first_is_held() {
        let mut touch = MenuTouch::default();
        let mut events = Vec::new();
        touch.step(&[f(1, 0.1, 0.2), f(2, 0.8, 0.9)], (1920, 1080), &mut events);
        touch.step(&[f(1, 0.1, 0.2), f(2, 0.8, 0.9)], (1920, 1080), &mut events);
        assert_eq!(events.len(), 1);
        touch.step(&[], (1920, 1080), &mut events);
        assert_eq!(events.len(), 2);
    }
}

#[cfg(test)]
mod tv_policy_tests {
    use super::*;

    #[test]
    fn tv_output_request_is_explicit_and_bounded() {
        let mut settings = store::Settings::default();
        assert_eq!(tv_request(&settings), (0, 0, 60));
        settings.width = 1920;
        settings.height = 1080;
        settings.refresh_hz = 120;
        assert_eq!(tv_request(&settings), (1920, 1080, 120));
        settings.width = 2560;
        settings.height = 1440;
        assert_eq!(tv_request(&settings), (2560, 1440, 120));
        settings.width = 3840;
        settings.height = 2160;
        assert_eq!(tv_request(&settings), (3840, 2160, 60));
        settings.match_window = true;
        assert_eq!(tv_request(&settings), (0, 0, 60));
        settings.match_window = false;
        settings.width = 1600;
        settings.height = 900;
        assert_eq!(tv_request(&settings), (0, 0, 60));
    }
}
