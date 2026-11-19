"""Checks the AArch64 bindgen C++ header preflight without an installed SDK."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'tooling' / 'prepare_bindgen.sh'


class BindgenPrepareTests(unittest.TestCase):
    @staticmethod
    def sdk(root):
        sdk = root / 'sdk'
        cxx = sdk / 'devkitA64/aarch64-none-elf/include/c++/16.1.0'
        triple = cxx / 'aarch64-none-elf'
        newlib = sdk / 'devkitA64/aarch64-none-elf/include'
        (triple / 'bits').mkdir(parents=True)
        newlib.mkdir(parents=True, exist_ok=True)
        for hdr in ['cassert', 'memory', 'vector']:
            (cxx / hdr).write_text('/* mock cross C++ header */\n')
        (triple / 'bits/c++config.h').write_text('/* mock target configuration */\n')
        (newlib / 'stdint.h').write_text('/* mock target stdint */\n')
        return sdk

    @staticmethod
    def run_it(root, sdk, *, compiler_ok=True):
        bin_dir = root / 'bin'
        bin_dir.mkdir()
        clang = bin_dir / 'clang++'
        clang.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$PF2_TEST_ARGS"\n' +
                         ('exit 0\n' if compiler_ok else 'echo "mock clang parse failure" >&2\nexit 1\n'))
        clang.chmod(0o755)
        env = dict(os.environ, DEVKITPRO=str(sdk), PF2_TEST_ARGS=str(root / 'args.txt'),
                   PATH=str(bin_dir) + os.pathsep + os.environ.get('PATH', ''))
        return subprocess.run(['bash', '-c', f'source "{SCRIPT}" && printf "ENV:%s\\n" "$PF2_BINDGEN_ARGS"'],
                              env=env, text=True, capture_output=True)

    def test_uses_cross_target_not_host_cpp_headers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.sdk(root)
            result = self.run_it(root, sdk)
            self.assertEqual(result.returncode, 0, result.stderr)
            args = (root / 'args.txt').read_text().splitlines()
            self.assertIn('--target=aarch64-none-elf', args)
            self.assertIn('-nostdinc++', args)
            self.assertIn('-std=c++20', args)
            self.assertIn('-isystem'+str(sdk/'devkitA64/aarch64-none-elf/include/c++/16.1.0'), args)
            self.assertIn('-isystem'+str(sdk/'devkitA64/aarch64-none-elf/include/c++/16.1.0/aarch64-none-elf'), args)
            self.assertIn('-isystem'+str(sdk/'devkitA64/aarch64-none-elf/include'), args)
            self.assertIn('Bindgen preflight: PASS', result.stdout)
            self.assertIn('ENV:--target=aarch64-none-elf', result.stdout)

    def test_missing_target_stdlib_fails_before_cargo(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.sdk(root)
            (sdk/'devkitA64/aarch64-none-elf/include/c++/16.1.0/cassert').unlink()
            result = self.run_it(root, sdk)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('no usable <cassert>', result.stderr)
            self.assertFalse((root/'args.txt').exists())

    def test_clang_preflight_failure_reports_target_header_problem(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sdk = self.sdk(root)
            result = self.run_it(root, sdk, compiler_ok=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('target C++ preflight FAILED', result.stderr)
            self.assertIn('mock clang parse failure', result.stderr)

    def test_cargo_receives_the_discovered_bindgen_flags(self):
        script = (ROOT/'tooling/build_official_rust.sh').read_text()
        self.assertIn('source "$HERE/tooling/prepare_bindgen.sh"', script)
        self.assertIn('BINDGEN_EXTRA_CLANG_ARGS_aarch64-nintendo-horizon=$PF2_BINDGEN_ARGS', script)
        self.assertIn('PF2_CARGO_TARGET_DIR', script)
        self.assertIn('ELF="$CARGO_TARGET_DIR/aarch64-nintendo-horizon/release/pf-nx"', script)


if __name__ == '__main__':
    unittest.main()
