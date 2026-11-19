//! What this device decodes and presents: one fact, read by everything that must agree about it.
//! `session::connect` advertises from it; `Settings::clamp_to_caps` normalises stored documents
//! to it. The platform installs it once at startup from `VideoDecoders::caps`; unset reads as
//! [`VideoCaps::SWITCH`].
use std::sync::OnceLock;

use crate::core::model::CodecPref;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoCaps {
    /// GPU wavelet decode on the shared device (`pf-client-video`).
    pub pyrowave: bool,
    pub h264: bool,
    pub hevc: bool,
    pub av1: bool,
    /// Main10 HEVC decodes and presents.
    pub ten_bit: bool,
    /// Can consume HDR10/PQ from the host. Some platforms use native HDR10
    /// scanout; the Switch 2's initial Main10 implementation instead
    /// tone-maps to its existing SDR Vulkan surface. Never confuse that
    /// compatibility path with a 10-bit HDR display mode.
    pub hdr: bool,
    /// Widest layout the audio output plays.
    pub max_channels: u8,
}

impl VideoCaps {
    /// The console's NVDEC lane: 8-bit H.264 and HEVC, SDR, stereo.
    pub const SWITCH: Self = Self {
        pyrowave: false,
        h264: true,
        hevc: true,
        av1: false,
        ten_bit: false,
        hdr: false,
        max_channels: 2,
    };

    /// The codec preferences worth offering, in display order. The one place the set is
    /// spelled, so the stored-document clamp and the advertised wire set agree.
    pub fn codec_prefs(self) -> Vec<CodecPref> {
        let mut prefs = Vec::with_capacity(4);
        if self.h264 || self.hevc || self.av1 {
            prefs.push(CodecPref::Auto);
        }
        if self.pyrowave {
            prefs.push(CodecPref::PyroWave);
        }
        if self.h264 {
            prefs.push(CodecPref::H264);
        }
        if self.hevc {
            prefs.push(CodecPref::Hevc);
        }
        if self.av1 {
            prefs.push(CodecPref::Av1);
        }
        prefs
    }
}

static INSTALLED: OnceLock<VideoCaps> = OnceLock::new();

/// Publish the platform's caps. Call once, before settings load; later calls are ignored.
pub fn install(caps: VideoCaps) {
    if INSTALLED.set(caps).is_err() {
        tracing::warn!("video caps already installed — ignoring {caps:?}");
    } else {
        tracing::info!("video caps: {caps:?}");
    }
}

pub fn video_caps() -> VideoCaps {
    INSTALLED.get().copied().unwrap_or(VideoCaps::SWITCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyrowave_alone_offers_no_automatic() {
        let caps = VideoCaps {
            pyrowave: true,
            h264: false,
            hevc: false,
            ..VideoCaps::SWITCH
        };
        assert_eq!(caps.codec_prefs(), [CodecPref::PyroWave]);
        assert_eq!(
            VideoCaps::SWITCH.codec_prefs(),
            [CodecPref::Auto, CodecPref::H264, CodecPref::Hevc]
        );
    }
}
