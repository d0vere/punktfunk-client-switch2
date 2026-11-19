"""Switch 2 H.264 GetFrame failure path regression guards.

This does not emulate real NVDEC; the native C mock independently exercises
SDK CopyFrame. The switch2 NRO still needs an on-console acceptance test.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]

class H264BlockLinearGuards(unittest.TestCase):
    def test_h264_8bit_uses_sdk_blocklinear_untiling(self):
        c = (ROOT / 'ports/switch2/native/pf2_decoder_switch2.c').read_text()
        self.assertIn('d->block_linear_8bit = settings->bit_depth == 8;', c)
        self.assertIn('.tiling = d->block_linear_8bit', c)
        self.assertIn('.color_policy = d->block_linear_8bit ? 0', c)
        self.assertIn('if (d->block_linear_8bit) {', c)
        self.assertIn('videoDecoderCopyFrame(&d->object, &d->frame,', c)
        self.assertNotIn('block_linear_8bit_av1', c)

    def test_native_mock_validates_h264_direct_and_fallback(self):
        mock = (ROOT / 'switch2/tests/test_direct_nv12_native.c').read_text()
        self.assertIn('assert(c->tiling == ViddecTilingMode_BlockLinear)', mock)
        self.assertIn('assert(h264_copy_calls == 2)', mock)

    def test_keep_av1_120_bitrate_and_hevc_gate(self):
        rs = (ROOT / 'crates/pf-nx-switch2-platform/src/video.rs').read_text()
        self.assertIn('const STREAM_MAX_FPS: u32 = 120', rs)
        self.assertIn('h264: true, hevc: false, av1: true', rs)
        self.assertIn('option_env!("PF2_DIRECT_NV12_OUTPUT")', rs)
        self.assertIn('matches!(self.spec.codec, quic::CODEC_AV1 | quic::CODEC_H264)', rs)

    def test_unused_ffi_and_test_only_helper_do_not_warn_in_release(self):
        ffi = (ROOT / 'crates/pf-nx-switch2-platform/src/ffi.rs').read_text()
        self.assertNotIn('pub fn pf2_h264_config_from_annexb', ffi)
        pic = (ROOT / 'crates/pf-nx-switch2-platform/src/video/source_picture.rs').read_text()
        self.assertIn('#[cfg(test)]\npub(super) fn h264_dimensions', pic)

if __name__ == '__main__':
    unittest.main()
