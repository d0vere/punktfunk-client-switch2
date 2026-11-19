"""Source-level invariants; these cannot replace Vulkan/SDK hardware tests."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NativeHardeningGuards(unittest.TestCase):
    def source(self, name):
        return (ROOT / name).read_text()

    def test_gpu_error_requires_full_sync_reinitialization(self):
        vulkan = self.source('ports/switch2/native/pf2_vulkan_switch2.c')
        self.assertIn('if (rc!=VK_SUCCESS) return broken_frame(p,rc);', vulkan)
        self.assertIn('p->device_lost=true;', vulkan)
        self.assertIn('return renew_frame_sync(p);', vulkan)
        self.assertIn('if (renew_frame_sync(p)!=PF2_OK)', vulkan)
        self.assertIn('if (extent.width < 2 || extent.height < 2', vulkan)
        self.assertIn('if (p->extent.width < 2 || p->extent.height < 2)', vulkan)

    def test_screenshots_and_optional_scanout(self):
        display = self.source('crates/pf-nx-switch2-platform/src/display.rs')
        self.assertIn('PF2_SCANOUT_PROBE', display)
        self.assertIn('surface.read_pixels(&info, &mut pixels, row, (0, 0))', display)
        self.assertIn('return Err(anyhow!("Ganesh Vulkan read_pixels failed', display)

    def test_low_memory_raster_and_smoothing(self):
        display = self.source('crates/pf-nx-switch2-platform/src/display.rs')
        video = self.source('crates/pf-nx-switch2-platform/src/video.rs')
        self.assertIn('1920*1080', display)
        self.assertIn('smoothing_enabled: AtomicBool', display)
        self.assertIn('_ => smoothing > 0', display)
        self.assertIn('bounded_video_handoff(spec.smoothing)', video)

    def test_main10_requires_opt_in(self):
        video = self.source('crates/pf-nx-switch2-platform/src/video.rs')
        self.assertIn('PF2_ENABLE_MAIN10', video)
        self.assertIn('ten_bit: main10, hdr: main10', video)

    def test_bounded_audio_and_boot_log(self):
        audio = self.source('ports/switch2/native/pf2_audio_switch2.c')
        boot = self.source('ports/switch2/native/pf2_runtime_switch2.c')
        self.assertIn('pcm_capacity', audio)
        self.assertNotIn('free(buf);', audio)
        self.assertIn('512 * 1024', boot)
        self.assertIn('rename(path,previous)', boot)

    def test_pads_and_decoder_contract(self):
        pads = self.source('crates/pf-nx-switch2-platform/src/input.rs')
        ffi = self.source('ports/switch2/native/pf2_native.h')
        self.assertIn('slot) >= self.rumble_seen.len()', pads)
        self.assertIn('staging buffers before releasing the SDK frame', ffi)

if __name__ == '__main__':
    unittest.main()
