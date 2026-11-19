"""Host-only tests for the Skia source preparation/launcher; no target claim."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SkiaPrepTests(unittest.TestCase):
    def test_c_cross_compiler_filters_incompatible_target_flag(self):
        for wrapper, compiler in [('skia-cc.sh', 'aarch64-none-elf-gcc'),
                                  ('skia-cxx.sh', 'aarch64-none-elf-g++')]:
            with self.subTest(wrapper=wrapper), tempfile.TemporaryDirectory() as d:
                root = Path(d)
                bindir = root/'bin'
                bindir.mkdir()
                fake = bindir/compiler
                fake.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$PF2_TEST_ARGS"\n')
                fake.chmod(0o755)
                sdk = root/'sdk'
                repo = root/'client'
                env = dict(os.environ, PATH=str(bindir)+os.pathsep+os.environ['PATH'],
                           DEVKITPRO=str(sdk), PF2_OFFICIAL_REPO=str(repo),
                           PF2_TEST_ARGS=str(root/'args.txt'), ARCH='-march=armv8-a')
                subprocess.run(['bash', str(ROOT/'tooling'/wrapper),
                                '--target=aarch64-nintendo-horizon', '-O2', '-c', 'test.c'],
                               env=env,check=True)
                got=(root/'args.txt').read_text().splitlines()
                self.assertNotIn('--target=aarch64-nintendo-horizon',got)
                self.assertIn('-D__SWITCH2__',got)
                self.assertIn('-ftls-model=local-exec',got)
                self.assertIn('-march=armv8-a',got)
                self.assertIn('test.c',got)
                self.assertIn(str(sdk/'libounce/include'),got)

    def test_wrong_skia_pin_aborts_before_download(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            repo=root/'official-client'
            (repo/'toolchain/skia').mkdir(parents=True)
            (repo/'toolchain/skia/horizon.patch').write_text('placeholder')
            (repo/'toolchain/skia/args.gn').write_text('ar = "aarch64-none-elf-ar"')
            (repo/'Cargo.lock').write_text('''version = 4
[[package]]
name = "skia-bindings"
version = "0.153.2"
''')
            env=dict(os.environ, PF2_WORK=str(root))
            result=subprocess.run(['bash',str(ROOT/'tooling/prepare_skia.sh')],
                                  env=env,capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('Wrong Skia pin',result.stderr)
            self.assertFalse((root/'skia-source').exists())

    def test_full_cross_build_has_real_skia_prepare_gate(self):
        build=(ROOT/'tooling/build_official_rust.sh').read_text()
        self.assertIn('source "$HERE/tooling/prepare_skia.sh"',build)
        self.assertIn('FORCE_SKIA_BUILD=1',(ROOT/'tooling/prepare_skia.sh').read_text())
        self.assertIn('env "BINDGEN_EXTRA_CLANG_ARGS_aarch64-nintendo-horizon=',build)
        self.assertIn('cargo +nightly-2026-10-04 build',build)
        self.assertIn('verify_artifacts.py',build)


if __name__ == '__main__':
    unittest.main()
