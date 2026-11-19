"""Offline source guards for hardware black-screen diagnosis; not console verification."""
from pathlib import Path
import importlib.util
import unittest

R = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('integration_for_black', R/'tooling/apply_integration.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

MENU = '''    let warm = Instant::now();
    app.display.frame(&mut |canvas| {
        // One flush for the whole tour; flush per frame if the console's memory peaks there.
        console.warm_up(canvas, &viewport, |_| {});
    })?;
    tracing::info!("console: warmed the tabs in {} ms", warm.elapsed().as_millis());
'''

class BlackScreenSourceGuards(unittest.TestCase):
    def test_target_specific_warmup_skip_and_switch1_preserved(self):
        new=module.patched('crates/pf-nx-app/src/runtime/menu.rs', MENU)
        self.assertIn('app.device.platform == "switch" && app.device.model == "Nintendo Switch 2"', new)
        self.assertIn('console.warm_up(canvas, &viewport, |_| {});', new)
        self.assertIn('} else {', new)
        self.assertIn('crates/pf-nx-app/src/runtime/menu.rs', module.CHANGES)

    def test_pl_u_system_fonts_hooked(self):
        platform=(R/'rust/crates/pf-nx-switch2-platform/src/lib.rs').read_text()
        manifest=(R/'rust/crates/pf-nx-switch2-platform/Cargo.toml').read_text()
        self.assertIn('fn fallback_fonts(&self)', platform)
        self.assertIn('pf_nx_sys::pl::shared_fonts("en")', platform)
        self.assertIn('fonts: OnceCell<Vec<&\'static [u8]>>', platform)
        self.assertIn('pf-nx-sys = { path = "../pf-nx-sys", default-features = false }', manifest)

    def test_visual_probe_preserves_real_official_ui(self):
        d=(R/'rust/crates/pf-nx-switch2-platform/src/display.rs').read_text()
        self.assertIn('PF2_SCANOUT_PROBE', d)
        self.assertIn('pf2_presenter_frame(presenter, pixels.as_ptr()', d)
        self.assertIn('draw_gpu(', d)
        self.assertIn('ffi::pf2_presenter_frame_gpu(self.presenter)', d)
        self.assertIn('self.compose(ui)?;', d)
        self.assertIn('blend_ui(&mut self.pixels', d)
        self.assertIn('nonblack-rgb=', d)
        self.assertIn('switch2 present: frame=', d)
        self.assertIn('surface.read_pixels(', d)
        native = (R/'native/pf2_vulkan_switch2.c').read_text()
        self.assertIn('renew_frame_sync(p)', native)
        self.assertIn('return broken_frame(p,rc)', native)

    def test_report_does_not_claim_hardware_fix(self):
        t=(R/'report/BLACK_SCREEN_DIAG.md').read_text()
        self.assertIn('50961 ms', t)
        self.assertIn("Non e' possibile dedurre", t)

if __name__ == '__main__': unittest.main()
