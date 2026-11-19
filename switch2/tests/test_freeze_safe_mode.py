"""Source guards for AV1-only negotiation preserving the user bitrate."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DiagnosticProfile(unittest.TestCase):
    def test_bitrate_codec_and_mode_are_explicitly_opt_in(self):
        s = (ROOT / 'crates/pf-nx-app/src/runtime/connect.rs').read_text()
        self.assertIn('PF2_SAFE_STREAM', s)
        self.assertNotIn('mode.refresh_hz = mode.refresh_hz.min(60)', s)
        self.assertIn('av1_only_preserves_manual_100mbps_and_120fps', s)
        self.assertIn('(1920, 1080, 120)', s)
        self.assertIn('(mode, bitrate_kbps, if enabled { CodecPref::Av1 } else { codec })', s)
        self.assertNotIn('(mode, 15_000, CodecPref::Av1)', s)
        self.assertIn('av1_only_stream_profile', s)
        self.assertIn('stream profile bitrate override', s)
        self.assertIn('normal_mode_preserves_all_user_settings', s)
        self.assertIn('automatic_bitrate_stays_automatic', s)

    def test_av1_is_exclusive_and_main10_disabled(self):
        s = (ROOT / 'crates/pf-nx-app/src/session/connect.rs').read_text()
        self.assertIn('let safe_av1 = matches!(option_env!("PF2_SAFE_STREAM")', s)
        self.assertIn('if safe_av1 { quic::CODEC_AV1 }', s)
        self.assertIn('let hdr = !safe_av1', s)
        self.assertIn('let ten_bit = hdr || (!safe_av1', s)


if __name__ == '__main__':
    unittest.main()
