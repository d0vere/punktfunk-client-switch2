"""Contracts for experimental AV1 direct NV12 staging (host only)."""
import pathlib
import unittest
ROOT = pathlib.Path(__file__).resolve().parents[2]

class DirectNv12OutputContract(unittest.TestCase):
    def source(self, path):
        return (ROOT / path).read_text()

    def test_native_lends_caller_buffer_and_restores_owned_pointers(self):
        c = self.source('switch2/native/pf2_decoder_switch2.c')
        self.assertIn('Pf2Error pf2_decoder_acquire_into(', c)
        self.assertIn('d->y = packed;', c)
        self.assertIn('d->uv = packed + d->luma_bytes;', c)
        self.assertIn('d->y = original_y;', c)
        self.assertIn('d->uv = original_uv;', c)
        self.assertIn('capacity - d->luma_bytes < d->chroma_bytes', c)
        self.assertIn('d->block_linear_8bit', c)
        self.assertIn('videoDecoderCopyFrame(', c)

    def test_path_opt_in_and_fallback_untouched(self):
        video = self.source('crates/pf-nx-switch2-platform/src/video.rs')
        self.assertIn('option_env!("PF2_DIRECT_NV12_OUTPUT")', video)
        self.assertIn('Some(self.nv12_pool.take(direct_bytes))', video)
        self.assertIn('ffi::pf2_decoder_acquire_into(', video)
        self.assertIn('ffi::pf2_decoder_acquire(decoder.handle, &mut f, 0)', video)
        self.assertIn('if direct_pack { pack_us = 0; }', video)
        self.assertIn('self.nv12_pool.give_back(unused)', video)
        self.assertIn('direct_nv12={}', video)
        self.assertIn('const STREAM_MAX_FPS: u32 = 120;', video)

    def test_direct_nv12_avoids_known_stable_rust_errors(self):
        # Regression: the first experimental patch built the C bridge but
        # failed Rust E0599/E0658 during the actual Switch 2 cross-build.
        video = self.source('crates/pf-nx-switch2-platform/src/video.rs')
        self.assertNotIn('const DIRECT_PACK: bool = matches!', video)
        self.assertIn('let direct_pack = matches!(option_env!("PF2_DIRECT_NV12_OUTPUT"), Some("1"))', video)
        self.assertIn('.ok_or_else(|| anyhow!("direct NV12 output dimensions overflow"))?', video)
        self.assertIn('self.spec.bit_depth == 8', video)
        self.assertIn('matches!(self.spec.codec, quic::CODEC_AV1 | quic::CODEC_H264)', video)

    def test_no_false_zero_copy_claim(self):
        doc = self.source('switch2/DIRECT_NV12_OUTPUT_EXPERIMENT.md')
        self.assertIn('**This is not zero-copy decoder-to-display.**', doc)
        display = self.source('crates/pf-nx-switch2-platform/src/display.rs')
        self.assertIn('skia_safe::Data::new_copy(&picture.nv12)', display)

if __name__ == '__main__':
    unittest.main()
