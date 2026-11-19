"""Host-only executable tests for guarded Git integration. No console assertions."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('pf2integration',ROOT/'tooling/apply_integration.py')
mod=importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

# Minimal text containing ALL exact upstream anchors. This checks our mutation
# logic, not that a complete GitHub repository was locally available.
ORIGINAL={
'Cargo.toml': '''[package]
name = "pf-nx"
version = "1.1.0"
[dependencies]
pf-nx-app = { path = "crates/pf-nx-app" }
[target.'cfg(target_os = "horizon")'.dependencies]
pf-nx-platform = { path = "crates/pf-nx-platform" }
pf-nx-sys = { path = "crates/pf-nx-sys" }
[workspace]
members = [
    "crates/pf-nx-probe",
]
[workspace.dependencies]
skia-safe = { version = "0.153", features = ["textlayout"] }
''',
'Cargo.lock': '''version = 4
[[package]]
name = "pf-nx"
version = "1.1.0"
dependencies = [
 "pf-nx-platform",
 "pf-nx-sys",
]
[[package]]
name = "pf-nx-sys"
version = "1.1.0"
''',
'src/main.rs': '''#[cfg(target_os = "horizon")]
fn main() {
    unreachable!();
}
#[cfg(not(target_os = "horizon"))]
fn main() { }
''',
'crates/pf-nx-sys/build.rs': '''fn main() {
    println!("cargo:rustc-check-cfg=cfg(pf_av)");
    let os = "horizon";
    if os != "horizon" {
        return;
    }
}
''',
'crates/pf-nx-sys/src/lib.rs': '''pub mod applet;
pub mod audout;
pub mod fb;
pub mod hid;
pub mod vi;
''',
'.cargo/config.toml': '''[env]
CFLAGS_aarch64_nintendo_horizon = "-isystem /opt/devkitpro/libnx/include"
''',
'crates/pf-nx-app/src/runtime/menu.rs': '''    let warm = Instant::now();
    app.display.frame(&mut |canvas| {
        // One flush for the whole tour; flush per frame if the console's memory peaks there.
        console.warm_up(canvas, &viewport, |_| {});
    })?;
    tracing::info!("console: warmed the tabs in {} ms", warm.elapsed().as_millis());
''',
'crates/pf-nx-app/src/core/model.rs': '''pub enum CodecPref {
    Auto,
    H264,
    Hevc,
}
''',
'crates/pf-nx-app/src/core/caps.rs': '''pub struct VideoCaps {
    pub h264: bool,
    pub hevc: bool,
}
impl VideoCaps {
    fn switch() -> Self {
        Self {
        h264: true,
        hevc: true,
        }
    }
    fn prefs(self) -> Vec<CodecPref> {
        let mut prefs=Vec::new();
        if self.h264 || self.hevc {
            prefs.push(CodecPref::Auto);
        }
        if self.hevc {
            prefs.push(CodecPref::Hevc);
        }
        prefs
    }
}
''',
'crates/pf-nx-app/src/session/connect.rs': '''fn codec(pref: CodecPref) -> u8 {
    match pref {
        CodecPref::Hevc => quic::CODEC_HEVC,
    }
}
''',
'crates/pf-nx-app/src/core/settings.rs': '''fn c(pref: CodecPref) -> &str {
    match pref {
        CodecPref::Hevc => "hevc",
    }
}
fn n(s:&str) -> CodecPref {
    match s {
        "hevc" => CodecPref::Hevc,
    }
}
''',
}

class IntegrationTests(unittest.TestCase):
    def test_patch_manifest_and_lock(self):
        m=tomllib.loads(mod.patched('Cargo.toml',ORIGINAL['Cargo.toml']))
        self.assertIn('pf-nx-switch2-platform', m['target']["cfg(target_os = \"horizon\")"]['dependencies'])
        self.assertIn('embed-freetype',m['workspace']['dependencies']['skia-safe']['features'])
        lock=tomllib.loads(mod.patched('Cargo.lock',ORIGINAL['Cargo.lock']))
        self.assertTrue(any(p['name']=='pf-nx-switch2-platform' for p in lock['package']))
        self.assertIn('av1',mod.patched('crates/pf-nx-app/src/core/settings.rs', ORIGINAL['crates/pf-nx-app/src/core/settings.rs']))
        sys_build=mod.patched('crates/pf-nx-sys/build.rs', ORIGINAL['crates/pf-nx-sys/build.rs'])
        self.assertIn('cargo:rustc-cfg=pf2_switch2',sys_build)
        self.assertIn('cargo:rustc-check-cfg=cfg(pf2_switch2)',sys_build)
        self.assertIn('cargo:rerun-if-env-changed=PF2_SWITCH2',sys_build)
        sys_lib=mod.patched('crates/pf-nx-sys/src/lib.rs', ORIGINAL['crates/pf-nx-sys/src/lib.rs'])
        self.assertIn('#[cfg(not(pf2_switch2))]\npub mod vi;',sys_lib)
        self.assertIn('#[cfg(not(pf2_switch2))]\npub mod fb;',sys_lib)

    def test_missing_anchor_fails(self):
        with self.assertRaises(ValueError): mod.patched('crates/pf-nx-app/src/core/model.rs','no enum here')

    def test_guarded_install_idempotence_and_rollback(self):
        with tempfile.TemporaryDirectory() as d:
            work=Path(d);repo=work/'official-client'
            repo.mkdir()
            subprocess.run(['git','init','-q'],cwd=repo,check=True)
            subprocess.run(['git','config','user.email','test@example.invalid'],cwd=repo,check=True)
            subprocess.run(['git','config','user.name','Test'],cwd=repo,check=True)
            for name, content in ORIGINAL.items():
                f=repo/name;f.parent.mkdir(parents=True,exist_ok=True);f.write_text(content)
            subprocess.run(['git','add','.'],cwd=repo,check=True)
            subprocess.run(['git','commit','-qm','fixture'],cwd=repo,check=True)
            sha=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip()
            saved=mod.PIN
            try:
                mod.PIN=sha
                mod.integrate(work)
                manifest=json.loads((work/'.pf2_integration.json').read_text())
                self.assertIn('Cargo.lock',manifest)
                self.assertIn('crates/pf-nx-sys/src/lib.rs',manifest)
                self.assertIn('crates/pf-nx-switch2-platform/src/video.rs',manifest)
                mod.integrate(work)
                self.assertIn('pf_nx_app::run(platform)',(repo/'src/main.rs').read_text())
                changed=repo/'crates/pf-nx-app/src/core/model.rs'
                changed.write_text(changed.read_text()+'// User change\n')
                with self.assertRaisesRegex(ValueError,'User-modified'):mod.integrate(work)
                subprocess.run(['python3',str(ROOT/'tooling/rollback_integrated.py'),'--work',str(work)],check=True,stdout=subprocess.PIPE)
                self.assertIn('// User change',changed.read_text())
                self.assertEqual((repo/'Cargo.toml').read_text(), ORIGINAL['Cargo.toml'])
                self.assertEqual((repo/'crates/pf-nx-sys/src/lib.rs').read_text(), ORIGINAL['crates/pf-nx-sys/src/lib.rs'])
            finally:mod.PIN=saved

if __name__=='__main__':unittest.main()
