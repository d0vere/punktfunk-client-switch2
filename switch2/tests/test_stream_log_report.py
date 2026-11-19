"""Tests for the dependency-free Switch 2 performance log analyzer."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / 'switch2' / 'tooling' / 'stream_log_report.py'
spec = importlib.util.spec_from_file_location('stream_log_report', SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class StreamLogReportTests(unittest.TestCase):
    def test_live_stream_records_and_pool_are_counted(self):
        log = '\n'.join([
            '2026-11-18T22:30:00Z INFO session::connect connected: codec=4 offered=4 wire_budget_kbps=100000',
            '2026-11-18T22:30:01Z INFO pf_nx_app::runtime::stream stats: present 10.0s: '
            '1200 frames, 0 missed vblanks, 1170 pictures, 30 unseen (30 replaced, 0 queue full), '
            'decode to present p50 11.5 p95 16.2 max 18.1 ms | '
            '1920×1080@120 · 120 fps · 88.2 Mb/s · target 100 Mb/s · AV1 8-bit',
            '2026-11-18T22:30:02Z INFO switch2 video displayed: ticks=120 distinct_frames=118 cached_gpu_copies=2',
            '2026-11-18T22:30:03Z INFO video pipeline: nv12_pool_reuses=22044 nv12_pool_allocations=3',
            '2026-11-18T22:30:04Z INFO data wire ingress packets=80000 video_kbps=85000 '
            'fec_repaired=0 frames_dropped=1 rejected=4700 max_gap_ms=23',
            '2026-11-18T22:30:05Z INFO recovery resuming after 23ms (frame 18001)',
            '2026-11-18T22:30:06Z INFO audio[10s]: underruns 1 silent, 0 audible; '
            'audio_ahead_ms=35-55 video_e2e_ms=14-31; a/v +40 ms smoothed',
        ])
        r = module.analyze(log)
        self.assertEqual(r['codec'], 4)
        self.assertEqual(r['welcome_budget_mbps'], 100)
        self.assertEqual(r['active_115fps_windows'], 1)
        self.assertEqual(r['active_mbps']['median'], 88.2)
        self.assertEqual(r['presenter_distinct_ge_115'], 1)
        self.assertEqual(r['nv12_pool']['reuses'], 22044)
        self.assertEqual(r['frames_dropped_ingress'], 1)
        self.assertEqual(r['packets_rejected_not_frame_losses'], 4700)
        self.assertEqual(r['max_recovery_ms'], 23)
        self.assertEqual(r['audible_underruns'], 0)
        self.assertEqual(r['audio_av_offset_median_ms'], 40)

    def test_idle_gap_does_not_get_mislabelled_as_freeze(self):
        r = module.analyze(
            '2026-11-18T22:30:01Z WARN data receive gap — silence_ms=9472 unpolled_ms=1 burst=32\n'
            '2026-11-18T22:30:02Z INFO switch2 video displayed: '
            'ticks=120 distinct_frames=0 cached_gpu_copies=120\n'
        )
        self.assertEqual(r['max_gap_ms'], 9472)
        self.assertEqual(r['active_115fps_windows'], 0)
        self.assertIsNone(r['nv12_pool'])
        self.assertIn('neither alone proves a visible freeze', module.format_report(r, 'sample'))
        # No assertion of a freeze merely because the host sent no video.
        self.assertIn('neither alone proves a visible freeze', r['note'])

    def test_present_cadence_separates_active_and_idle_windows(self):
        log = '\n'.join([
            'INFO switch2 video displayed: ticks=120 distinct_frames=119 '
            'present_gap_samples=120 present_gap_over_1p5x=3 '
            'present_gap_over_2x=1 present_gap_max_us=18750',
            'INFO switch2 video displayed: ticks=120 distinct_frames=0 '
            'present_gap_samples=120 present_gap_over_1p5x=2 '
            'present_gap_over_2x=1 present_gap_max_us=34000',
        ])
        result = module.analyze(log)
        self.assertEqual(result['present_cadence_windows'], 2)
        self.assertEqual(result['present_gap_samples'], 240)
        self.assertEqual(result['present_gap_over_1p5x'], 5)
        self.assertEqual(result['present_gap_over_2x'], 2)
        self.assertEqual(result['present_gap_max_ms'], 34.0)
        self.assertEqual(result['present_active_windows'], 1)
        self.assertEqual(result['present_active_gap_over_2x'], 1)

    def test_previous_logs_without_cadence_fields_are_accepted(self):
        result = module.analyze('INFO switch2 video displayed: ticks=120 distinct_frames=118')
        self.assertEqual(result['presenter_window_samples'], 1)
        self.assertEqual(result['present_cadence_windows'], 0)
        self.assertIsNone(result['present_gap_max_ms'])

    def test_no_stream_log_is_supported(self):
        r = module.analyze('2026-11-18T22:30:00Z INFO menu loaded\n')
        self.assertIsNone(r['active_mbps']['median'])
        self.assertEqual(r['reported_receive_gaps'], 0)
        self.assertEqual(r['error_lines'], 0)


if __name__ == '__main__':
    unittest.main()
