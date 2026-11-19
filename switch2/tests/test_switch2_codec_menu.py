"""Switch 2 exposes only decoder-backed choices in the official UI."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class Switch2CodecMenu(unittest.TestCase):
    def test_switch2_ui_hides_hevc_without_changing_switch1(self):
        source = (ROOT / 'vendor/punktfunk/crates/client/pf-console-ui/src/screens/settings/rows.rs').read_text()
        self.assertIn('const CODECS_SWITCH2: [(&str, Label); 3] = [CODECS[0], CODECS[2], CODECS[3]];', source)
        self.assertIn('platform == crate::platform::Platform::Switch && av1_ok', source)
        self.assertIn('&CODECS_SWITCH2', source)
        self.assertIn('crate::platform::Platform::Ps5 | crate::platform::Platform::Switch => &CODECS_PS5', source)

    def test_switch2_does_not_advertise_hevc_in_handshake(self):
        source = (ROOT / 'crates/pf-nx-switch2-platform/src/video.rs').read_text()
        self.assertIn('h264: true, hevc: false, av1: true', source)
        source = (ROOT / 'crates/pf-nx-app/src/session/connect.rs').read_text()
        self.assertIn('let codecs = caps.codec_prefs()', source)
        self.assertIn('codecs.iter().fold(0', source)

    def test_unsupported_saved_codec_is_clamped(self):
        source = (ROOT / 'crates/pf-nx-app/src/core/settings.rs').read_text()
        self.assertIn('if !codecs.contains(&codec)', source)
        self.assertIn('self.set_codec_pref(first)', source)


if __name__ == '__main__':
    unittest.main()
