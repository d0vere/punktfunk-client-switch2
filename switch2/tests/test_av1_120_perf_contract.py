"""Host-only guards for the conservative AV1/120 performance follow-up.

These tests protect invariants in the Switch 2 source, not hardware behavior.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class StreamingPerformanceContract(unittest.TestCase):
    def source(self, rel):
        return (ROOT / rel).read_text()

    def test_frame_buffers_recycled_after_last_owner_drops(self):
        display = self.source('crates/pf-nx-switch2-platform/src/display.rs')
        video = self.source('crates/pf-nx-switch2-platform/src/video.rs')
        pool = self.source('crates/pf-nx-switch2-platform/src/nv12_pool.rs')
        self.assertIn('impl Drop for VideoPicture', display)
        self.assertIn('std::mem::take(&mut self.nv12)', display)
        self.assertIn('self.nv12_pool.take(y_bytes + uv_bytes)', video)
        self.assertIn('nv12_pool: Arc::clone(&self.nv12_pool)', video)
        self.assertIn('MAX_CACHED_BUFFERS: usize = 2', pool)
        self.assertIn('MAX_CACHED_BYTES: usize = 32 * 1024 * 1024', pool)

    def test_video_is_not_downclocked_or_switched_to_h264(self):
        connect = self.source('crates/pf-nx-app/src/runtime/connect.rs')
        video = self.source('crates/pf-nx-switch2-platform/src/video.rs')
        self.assertIn('av1_only_stream_profile', connect)
        self.assertIn('(mode, bitrate_kbps, if enabled { CodecPref::Av1 } else { codec })', connect)
        self.assertIn('const STREAM_MAX_FPS: u32 = 120;', video)
        self.assertIn('PF2_ENABLE_MAIN10', video)

    def test_renderer_keeps_synchronous_copy_ownership(self):
        display = self.source('crates/pf-nx-switch2-platform/src/display.rs')
        self.assertIn('skia_safe::Data::new_copy(&picture.nv12)', display)
        self.assertIn('reuse_unchanged_video(', display)
        self.assertIn('self.video.replaced.fetch_add(1, Ordering::Relaxed)', display)

    def test_audio_offsets_remain_observational(self):
        audio = self.source('crates/pf-nx-app/src/session/audio_out.rs')
        self.assertIn('audio_ahead_ms=', audio)
        self.assertIn('video_e2e_ms=', audio)
        self.assertIn('unknown device output latency', audio)
        self.assertIn('r.sync.observe(o)', audio)
        self.assertNotIn('adjust_audio_playout(', audio)


if __name__ == '__main__':
    unittest.main()
