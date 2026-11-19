"""Negative tests for structural gates; a file suffix never passes validation."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

P = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('verify', P / 'tooling' / 'verify_artifacts.py')
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class ArtifactGuards(unittest.TestCase):
    def test_empty_fake_elf_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / 'fake.elf'
            p.write_bytes(b'\x7fELF' + b'\0' * 256)
            with self.assertRaises(ValueError):
                verify.check_elf(p)

    def test_renamed_bin_is_not_nro(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / 'fake.nro'
            p.write_bytes(b'ELF' + b'\0' * 4096)
            with self.assertRaises(ValueError):
                verify.check_nro(p)

    def test_manifest_is_explicit_about_incomplete(self):
        import json
        data = json.loads((P / 'manifest.json').read_text())
        self.assertEqual(data['qualification'], 'SOURCE_PATCH_ONLY__HARDWARE_VALIDATION_REQUIRED')
        self.assertIn('USER_CROSS_BUILD_LINKS_AND_PACKAGES_REAL_NRO', data['rust_frontend_integration'])
        self.assertIsNone(data['native_nro'])
        self.assertIn('UNTESTED_ON_DEVICE', data['hardware_test'])
