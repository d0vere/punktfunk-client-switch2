"""H.264 framing/recovery and HEVC gating in the Switch 2 native client."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]

class H264CodecContract(unittest.TestCase):
    def test_h264_bootstraps_on_idr_using_bounded_sps_and_pps(self):
        source = (ROOT / 'crates/pf-nx-switch2-platform/src/video/source_picture.rs').read_text()
        self.assertIn('const MAX_CONFIG_NAL: usize = 2048', source)
        self.assertIn('pub(super) fn observe', source)
        self.assertIn('if !has_idr { return H264Decision::NeedKeyframe; }', source)
        self.assertIn('pub(super) fn invalidate', source)
        self.assertIn('fn h264_bootstrap_collects_separate_sps_pps_and_starts_on_idr', source)
        self.assertIn('fn h264_parameter_change_does_not_reopen_on_inter_frame', source)

    def test_direct_nv12_accepts_av1_and_h264_without_advertising_hevc(self):
        source = (ROOT / 'crates/pf-nx-switch2-platform/src/video.rs').read_text()
        self.assertIn('matches!(self.spec.codec, quic::CODEC_AV1 | quic::CODEC_H264)', source)
        self.assertIn('option_env!("PF2_DIRECT_NV12_OUTPUT")', source)
        self.assertNotIn('option_env!("PF2_DIRECT_H264_OUTPUT")', source)
        self.assertIn('h264: true, hevc: false, av1: true', source)
        source = (ROOT / 'ports/switch2/native/pf2_decoder_switch2.c').read_text()
        self.assertIn('d->bit_depth != 8) return PF2_UNSUPPORTED;', source)
        self.assertIn('d->y = original_y;', source)
        self.assertIn('d->uv = original_uv;', source)

    def test_av1_only_diagnostic_remains_opt_in(self):
        source = (ROOT / 'crates/pf-nx-app/src/session/connect.rs').read_text()
        self.assertIn('let safe_av1 = matches!(option_env!("PF2_SAFE_STREAM"), Some("1"))', source)
        self.assertIn('if safe_av1 { quic::CODEC_AV1 }', source)

if __name__ == '__main__':
    unittest.main()
