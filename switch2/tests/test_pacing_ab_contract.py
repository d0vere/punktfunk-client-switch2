"""Host-only source guards for the reversible Switch 2 frame pacing A/B."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PacingABContractTests(unittest.TestCase):
    def test_override_respects_default_and_explicit_modes(self):
        display = (ROOT / 'crates/pf-nx-switch2-platform/src/display.rs').read_text()
        video = (ROOT / 'crates/pf-nx-switch2-platform/src/video.rs').read_text()
        self.assertIn('select_bounded_video_handoff(override_value: Option<&str>, smoothing: usize)', display)
        self.assertIn('Some("0") => false', display)
        self.assertIn('Some("1") => true', display)
        self.assertIn('_ => smoothing > 0', display)
        self.assertIn('bounded_video_handoff(spec.smoothing)', video)
        self.assertIn('requested_smoothing=', video)
        self.assertIn('self.frames.frame.lock()', video)  # clear before policy changes

    def test_observability_is_passive_and_bounded(self):
        display = (ROOT / 'crates/pf-nx-switch2-platform/src/display.rs').read_text()
        self.assertIn('struct PresentCadence', display)
        self.assertIn('self.present_cadence.note(Instant::now(), self.refresh_hz)', display)
        self.assertIn('present_gap_over_2x=', display)
        self.assertIn('take_samples()', display)
        self.assertIn('self.present_cadence = PresentCadence::default()', display)
        self.assertIn('PF2_BOUNDED_PACING', display)
        self.assertNotIn('std::thread::sleep(', display.split('struct PresentCadence')[1].split('struct FramePhase')[0])

    def test_av1_120_and_bitrate_contract_unchanged(self):
        connect = (ROOT / 'crates/pf-nx-app/src/runtime/connect.rs').read_text()
        video = (ROOT / 'crates/pf-nx-switch2-platform/src/video.rs').read_text()
        self.assertIn('av1_only_stream_profile', connect)
        self.assertIn('user bitrate=', connect)
        self.assertIn('const STREAM_MAX_FPS: u32 = 120;', video)
        self.assertIn('nv12_pool_reuses=', video)


if __name__ == '__main__':
    unittest.main()
