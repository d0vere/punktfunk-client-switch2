"""Switch1 legacy default window exports must not collide with libounce.

Structural tests on the guarded overlay; NOT an AArch64 linker run.
"""
import importlib.util
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('link_overlay', ROOT/'tooling/apply_integration.py')
mod=importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

SYS_SRC='//! Legacy libnx shim\npub mod fb;\npub mod pl;\npub mod vi;\n'
BUILD_SRC='fn main() {\n    println!("cargo:rustc-check-cfg=cfg(pf_av)");\n    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();\n    if os != "horizon" {\n        return;\n    }\n    println!("original unchanged for Switch 1");\n}\n'

class LinkViGuards(unittest.TestCase):
    def test_switch1_vi_fb_still_present_when_cfg_off(self):
        s=mod.patched('crates/pf-nx-sys/src/lib.rs',SYS_SRC)
        self.assertIn('#[cfg(not(pf2_switch2))]\npub mod vi;',s)
        self.assertIn('#[cfg(not(pf2_switch2))]\npub mod fb;',s)
        self.assertIn('pub mod pl;',s)
        self.assertEqual(s.count('pub mod vi;'),1)
        self.assertEqual(s.count('pub mod fb;'),1)

    def test_target_only_enables_cfg_in_switch2_build(self):
        s=mod.patched('crates/pf-nx-sys/build.rs',BUILD_SRC)
        self.assertIn('if os != "horizon" { return; }',s)
        self.assertIn('if env::var_os("PF2_SWITCH2").is_some() {',s)
        self.assertIn('println!("cargo:rustc-cfg=pf2_switch2");',s)
        self.assertIn('println!("cargo:rerun-if-env-changed=PF2_SWITCH2");',s)
        self.assertLess(s.index('if os != "horizon"'),s.index('if env::var_os("PF2_SWITCH2")'))

    def test_old_vi_file_left_untouched(self):
        self.assertNotIn('crates/pf-nx-sys/src/vi.rs',mod.CHANGES)
        self.assertIn('crates/pf-nx-sys/src/lib.rs',mod.CHANGES)

    def test_duplicate_export_names_are_documented(self):
        note=(ROOT/'report/LINK_VI_FIX.md').read_text()
        for symbol in ('nwindowGetDefault','__nx_win_init','__nx_win_exit'):
            self.assertIn(symbol,note)

if __name__=='__main__': unittest.main()
