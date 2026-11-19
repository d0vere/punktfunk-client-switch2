//! A launch on its way to a stream: the connect thread, what it dialled, and the plan for
//! leaving the title afterwards.

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use punktfunk_core::config::{CompositorPref, Mode};
use punktfunk_core::quic::v2::hello::ScreenWish;

use crate::core::model::{CodecPref, ConnectTarget};
use crate::core::settings::NxSettings;
use crate::platform::PadKind;
use crate::services::power::ExitPlan;
use crate::services::store::console::ConsoleStore;
use crate::services::store::{self, shared};
use crate::session::{self, Media};
use crate::ui::Service;

/// A connect in flight. Dropping it cancels the attempt and winds a landed session down off the
/// calling thread.
pub(super) struct PendingConnect {
    handle: Option<std::thread::JoinHandle<Result<session::Connected>>>,
    attempt: Arc<session::ConnectAttempt>,
}

impl PendingConnect {
    pub(super) fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(std::thread::JoinHandle::is_finished)
    }

    pub(super) fn join(mut self) -> std::thread::Result<Result<session::Connected>> {
        self.handle.take().expect("pending connect handle").join()
    }
}

impl Drop for PendingConnect {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else { return };
        self.attempt.cancel();
        let joiner = std::thread::spawn(move || {
            if let Ok(Ok(connected)) = handle.join() {
                super::teardown(connected);
            }
        });
        let mut abandoned = ABANDONED.lock().unwrap_or_else(PoisonError::into_inner);
        abandoned.retain(|j| !j.is_finished());
        abandoned.push(joiner);
    }
}

/// Joiners left by dropped connects. Only the exit waits on them: the host refuses a power
/// action while a session it holds for us is still live.
static ABANDONED: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

pub(super) fn await_abandoned() {
    let abandoned = std::mem::take(&mut *ABANDONED.lock().unwrap_or_else(PoisonError::into_inner));
    for joiner in abandoned {
        let _ = joiner.join();
    }
}

/// A launch handed from the menu to the stream loop.
pub(super) struct ConnectOutcome {
    pub connected: Result<session::Connected>,
    /// What was dialled, so a lost link can be dialled again.
    pub target: ConnectTarget,
    pub settings: store::Settings,
    pub exit_plan: Option<ExitPlan>,
}

/// Whether the document's size says "Native" (`0`): the stream then follows the console's output.
pub(super) fn follows_output(settings: &store::Settings) -> bool {
    settings.match_window || settings.width == 0 || settings.height == 0
}

/// The mode to ask for: the document's explicit pick, or the console's own output where it
/// says "Native" (`0`). The host refuses a 0×0 request.
pub(super) fn stream_mode(settings: &store::Settings, native: Mode) -> Mode {
    let explicit = !follows_output(settings);
    Mode {
        width: if explicit { settings.width } else { native.width },
        height: if explicit { settings.height } else { native.height },
        refresh_hz: if !explicit || settings.refresh_hz == 0 {
            native.refresh_hz
        } else {
            settings.refresh_hz
        },
    }
}

/// Limit only genuinely unsupported stream modes. A 60-fps libounce decoder
/// resource hint must not down-negotiate a 120-fps encoded stream.
pub(super) fn decoder_stream_mode(
    settings: &store::Settings, native: Mode, max_fps: u32,
) -> Mode {
    let mut mode = stream_mode(settings, native);
    if mode.refresh_hz > max_fps && max_fps > 0 {
        tracing::warn!(
            "stream: {}x{}@{} requested; hardware decoder supports {} fps here, using {}",
            mode.width, mode.height, mode.refresh_hz, max_fps, max_fps,
        );
        mode.refresh_hz = max_fps;
    }
    mode
}

/// AV1-only compatibility profile. The launch's bitrate and mode are never
/// rewritten: a manual 100 Mb/s must reach Hello as 100_000 kbps, and 120 Hz
/// must remain 120 Hz. `PF2_SAFE_STREAM=1` only selects the tested 8-bit codec.
fn av1_only_stream_profile(
    mode: Mode, bitrate_kbps: u32, codec: CodecPref, enabled: bool,
) -> (Mode, u32, CodecPref) {
    (mode, bitrate_kbps, if enabled { CodecPref::Av1 } else { codec })
}

/// Starts the connect on its own thread.
pub(super) fn spawn(
    media: &Media,
    identity: (String, String),
    target: ConnectTarget,
    settings: store::Settings,
    native: Mode,
) -> Result<PendingConnect> {
    let attempt = Arc::new(session::ConnectAttempt::default());
    let worker_attempt = attempt.clone();
    let media = media.clone();
    std::thread::Builder::new()
        .name("pf-connect".into())
        .spawn(move || {
            let requested = stream_mode(&settings, native);
            let max_fps = media.video.max_stream_fps(requested.width, requested.height);
            let (mode, bitrate_kbps, codec) = av1_only_stream_profile(
                decoder_stream_mode(&settings, native, max_fps),
                settings.bitrate_kbps,
                settings.codec_pref(),
                matches!(option_env!("PF2_SAFE_STREAM"), Some("1")),
            );
            if matches!(option_env!("PF2_SAFE_STREAM"), Some("1")) {
                tracing::warn!(
                    "PF2_SAFE_STREAM AV1-only enabled: requesting {}x{}@{}, 8-bit AV1, user bitrate={} kb/s; \
                     host capture resolution may differ",
                    mode.width, mode.height, mode.refresh_hz, bitrate_kbps,
                );
            }
            tracing::info!("requesting {}x{}@{} bitrate_kbps={}",
                mode.width, mode.height, mode.refresh_hz, bitrate_kbps);
            session::connect(
                &session::ConnectParams {
                    host: target.host,
                    port: target.port,
                    mode,
                    bitrate_kbps,
                    pyrowave_bpp_x100: settings.pyrowave_bpp_x100(),
                    hdr_enabled: !matches!(option_env!("PF2_SAFE_STREAM"), Some("1")) && settings.hdr_enabled,
                    ten_bit_sdr: !matches!(option_env!("PF2_SAFE_STREAM"), Some("1")) && settings.ten_bit_sdr,
                    audio_channels: settings.audio_channels,
                    keep_host_audio: settings.keep_host_audio,
                    identity,
                    pin: Some(target.fingerprint),
                    launch: target.launch,
                    profile: target.profile,
                    timeout: crate::services::budget::CONNECT,
                    codec,
                    compositor: CompositorPref::from_name(&settings.compositor).unwrap_or(CompositorPref::Auto),
                    gamepad_type: settings.gamepad_type(),
                    // Pad-only: the host draws the cursor.
                    cursor_capture: true,
                    pad_audio_caps: 0,
                    present_priority: settings.present_priority(),
                    // A spelling the wire can't carry is no wish.
                    screen_wish: ScreenWish::parse(&settings.host_screen),
                },
                &media,
                &worker_attempt,
            )
        })
        .map(|handle| PendingConnect {
            handle: Some(handle),
            attempt,
        })
        .context("spawn connect thread")
}

/// What the console committed to launch.
pub(super) struct Launch {
    pub addr: String,
    pub port: u16,
    pub fp_hex: String,
    pub launch: Option<String>,
    pub preset: Option<String>,
    pub profile: Option<String>,
}

/// A connect in flight with what it dialled and the session settings.
pub(super) type Started = (PendingConnect, ConnectTarget, store::Settings);

/// Resolves a launch against the document and starts its connect. `pad` is the first pad in
/// hand, which "Automatic" names as the session's default kind.
pub(super) fn start(
    store: &Arc<ConsoleStore>,
    media: &Media,
    identity: &(String, String),
    want: Launch,
    native: Mode,
    overlay: Option<&pf_client_core::presets::SettingsOverlay>,
    pad: Option<PadKind>,
) -> Result<Started> {
    let state = store.snapshot();
    let known = state
        .known_hosts
        .iter()
        .find(|h| h.addr == want.addr && h.port == want.port);
    let fingerprint = known
        .and_then(crate::core::model::KnownHost::fingerprint)
        .or_else(|| shared::parse_fp(&want.fp_hex))
        .context("that host isn't paired with this console yet")?;
    let mut settings = shared::launch_settings(
        &state,
        &want.addr,
        want.port,
        want.launch.as_deref(),
        want.preset.as_deref(),
    );
    if let Some(overlay) = overlay {
        settings = overlay.apply(&settings);
        settings.clamp_to_caps();
        tracing::info!(
            "test run: settings for this run codec={} bitrate_kbps={} {}x{}@{} pyrowave_bpp={} hdr={}",
            settings.codec,
            settings.bitrate_kbps,
            settings.width,
            settings.height,
            settings.refresh_hz,
            settings.pyrowave_bpp,
            settings.hdr_enabled,
        );
    }
    if settings.bitrate_kbps != state.settings.bitrate_kbps {
        tracing::info!(
            "stream profile bitrate override: global_kbps={} effective_kbps={}",
            state.settings.bitrate_kbps, settings.bitrate_kbps,
        );
    }
    // Resolve Automatic for Hello without losing the player's actual Automatic selection:
    // the input loop must still follow a Pro/Joy-Con swap after the stream starts.
    let handshake_settings = gamepad_handshake_settings(&settings, pad);
    let target = ConnectTarget {
        host: want.addr,
        port: want.port,
        fingerprint,
        launch: want.launch,
        profile: want.profile,
    };
    let handle = spawn(media, identity.clone(), target.clone(), handshake_settings, native)?;
    Ok((handle, target, settings))
}

pub(super) fn gamepad_handshake_settings(
    settings: &store::Settings, pad: Option<PadKind>,
) -> store::Settings {
    let mut handshake = settings.clone();
    if let Some(pad) = pad {
        handshake.set_gamepad_type(super::pads::resolve_kind(settings.gamepad_type(), pad));
    }
    handshake
}

#[cfg(test)]
mod safe_stream_tests {
    use super::*;

    #[test]
    fn av1_only_preserves_manual_100mbps_and_120fps() {
        let requested = Mode { width: 1920, height: 1080, refresh_hz: 120 };
        let (mode, bitrate, codec) = av1_only_stream_profile(
            requested, 100_000, CodecPref::Auto, true,
        );
        assert_eq!((mode.width, mode.height, mode.refresh_hz), (1920, 1080, 120));
        assert_eq!(bitrate, 100_000);
        assert_eq!(codec, CodecPref::Av1);
    }

    #[test]
    fn av1_only_preserves_resolution_and_other_fixed_rates() {
        for bitrate in [8_000, 15_000, 40_000, 60_000, 100_000] {
            let requested = Mode { width: 2560, height: 1440, refresh_hz: 120 };
            let (mode, sent, codec) = av1_only_stream_profile(
                requested, bitrate, CodecPref::H264, true,
            );
            assert_eq!((mode.width, mode.height, mode.refresh_hz), (2560, 1440, 120));
            assert_eq!(sent, bitrate);
            assert_eq!(codec, CodecPref::Av1);
        }
    }

    #[test]
    fn automatic_bitrate_stays_automatic() {
        let requested = Mode { width: 1920, height: 1080, refresh_hz: 120 };
        let (mode, sent, codec) = av1_only_stream_profile(
            requested, 0, CodecPref::Auto, true,
        );
        assert_eq!(mode.refresh_hz, 120);
        assert_eq!(sent, 0);
        assert_eq!(codec, CodecPref::Av1);
    }

    #[test]
    fn normal_mode_preserves_all_user_settings() {
        let requested = Mode { width: 2560, height: 1440, refresh_hz: 120 };
        let (mode, bitrate, codec) = av1_only_stream_profile(
            requested, 100_000, CodecPref::H264, false,
        );
        assert_eq!((mode.width, mode.height, mode.refresh_hz), (2560, 1440, 120));
        assert_eq!(bitrate, 100_000);
        assert_eq!(codec, CodecPref::H264);
    }
}

#[cfg(test)]
mod switch_pad_tests {
    use super::*;
    use crate::core::model::GamepadType;

    #[test]
    fn automatic_hello_mirrors_physical_pad_but_settings_stay_automatic() {
        let s = store::Settings::default();
        assert_eq!(s.gamepad_type(), GamepadType::Auto);
        assert_eq!(gamepad_handshake_settings(&s, Some(PadKind::SwitchPro)).gamepad_type(), GamepadType::SwitchPro);
        assert_eq!(gamepad_handshake_settings(&s, Some(PadKind::JoyConPair)).gamepad_type(), GamepadType::JoyConPair);
        assert_eq!(s.gamepad_type(), GamepadType::Auto);
    }

    #[test]
    fn forced_type_is_not_overridden_by_connected_hardware() {
        let mut s = store::Settings::default();
        s.set_gamepad_type(GamepadType::SwitchPro);
        assert_eq!(gamepad_handshake_settings(&s, Some(PadKind::JoyConPair)).gamepad_type(), GamepadType::SwitchPro);
    }
}

/// What to do to the selected host on the way out: only a host that answered its last
/// reachability check, since a host already down costs the whole budget.
pub(super) fn exit_plan(service: &Service, identity: &(String, String)) -> Option<ExitPlan> {
    let state = service.store.snapshot();
    let (host, port) = state.selected_host.clone()?;
    let known = state.known_hosts.iter().find(|h| h.addr == host && h.port == port)?;
    known.exit_action.action_id()?;
    if !service.is_online(known) {
        tracing::debug!("exit action skipped: {host} was not reachable");
        return None;
    }
    Some(ExitPlan {
        addr: known.addr.clone(),
        mgmt_port: known.mgmt_port.unwrap_or(crate::services::library::DEFAULT_MGMT_PORT),
        identity: identity.clone(),
        pin: Some(known.fingerprint()?),
        action: known.exit_action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Native" (0) dials the console's output; an explicit pick dials itself.
    #[test]
    fn a_native_mode_dials_the_output() {
        let output = Mode {
            width: 1280,
            height: 720,
            refresh_hz: 60,
        };
        let mut s = store::Settings::default();
        assert_eq!(stream_mode(&s, output), output);
        s.width = 1920;
        s.height = 1080;
        s.refresh_hz = 60;
        assert_eq!(
            stream_mode(&s, output),
            Mode {
                width: 1920,
                height: 1080,
                refresh_hz: 60
            }
        );
    }

    #[test]
    fn native_always_follows_the_current_panel_or_tv_refresh() {
        let handheld = Mode { width: 1920, height: 1080, refresh_hz: 120 };
        let tv = Mode { width: 3840, height: 2160, refresh_hz: 60 };
        let mut settings = store::Settings::default();
        // Even a stale explicit 60-fps setting must not override Native.
        settings.refresh_hz = 60;
        assert_eq!(stream_mode(&settings, handheld), handheld);
        assert_eq!(stream_mode(&settings, tv), tv);
        // In an explicitly selected resolution the FPS remains independent.
        settings.width = 1280;
        settings.height = 720;
        assert_eq!(stream_mode(&settings, handheld).refresh_hz, 60);
    }

    #[test]
    fn stream_mode_caps_decoder_without_changing_settings() {
        let native = Mode { width: 1280, height: 720, refresh_hz: 120 };
        let mut settings = store::Settings::default();
        settings.refresh_hz = 0;
        assert_eq!(decoder_stream_mode(&settings, native, 60).refresh_hz, 60);
        settings.refresh_hz = 120;
        assert_eq!(decoder_stream_mode(&settings, native, 60).refresh_hz, 60);
        assert_eq!(settings.refresh_hz, 120);
        assert_eq!(decoder_stream_mode(&settings, native, u32::MAX).refresh_hz, 120);
    }
}
