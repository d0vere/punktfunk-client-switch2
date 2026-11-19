"""Source/build contract for the pinned Skia newlib file I/O shim.

Not a substitute for cross-linking rust-lld or a Switch 2 runtime test.
"""
from pathlib import Path
import re
import unittest

P = Path(__file__).resolve().parents[1]

class SkiaLinkTests(unittest.TestCase):
    def test_static_native_archive_contains_compat_member(self):
        script=(P/'tooling/build_native.sh').read_text()
        self.assertIn('pf2_skia_compat; do', script)
        self.assertIn('libpf2native.a', script)
        self.assertIn('SKIA_COMPAT_INCLUDE', script)
        self.assertIn('official-client/toolchain/skia/include', script)
        self.assertIn('sys/mman.h', script)

    def test_weak_posix_exports_are_named_exactly(self):
        source=(P/'native/pf2_skia_compat.c').read_text()
        for name in ('pread','mmap','munmap','posix_madvise'):
            self.assertRegex(source, rf'__attribute__\(\(weak\)\)\s+[^\n]*\b{name}\s*\(')
        self.assertIn('void pf2_skia_compat_link_guard(void)', source)
        self.assertNotIn('MoonlightPlatform', source)
        self.assertIn('__SWITCH2__', source)

    def test_archive_extraction_anchor_is_used(self):
        ffi=(P/'rust/crates/pf-nx-switch2-platform/src/ffi.rs').read_text()
        impl=(P/'rust/crates/pf-nx-switch2-platform/src/lib.rs').read_text()
        self.assertIn('pub fn pf2_skia_compat_link_guard();',ffi)
        self.assertIn('unsafe { ffi::pf2_skia_compat_link_guard() };',impl)
        self.assertIn('#[link(name = "pf2native", kind = "static")]',ffi)

    def test_posix_compat_is_not_in_switch1_sys(self):
        overlay=(P/'tooling/apply_integration.py').read_text()
        self.assertNotIn('crates/pf-nx-sys/src/vi.rs',overlay)
        self.assertNotIn('crates/pf-nx-sys/src/rt.rs',overlay)
        self.assertIn('pf2_switch2',overlay)

    def test_all_four_host_c_tests_are_executed(self):
        s=(P/'build_full.sh').read_text()
        self.assertIn('"$WORK/test_ffi_layout"', s)
        self.assertIn('"$WORK/test_skia_compat"', s)
        self.assertIn('for unit in nv12 h264', s)

    def test_errors_are_not_silenced(self):
        source=(P/'native/pf2_skia_compat.c').read_text()
        self.assertIn('errno = ENOSYS;', source)
        self.assertIn('errno = EINVAL;', source)
        self.assertIn('const int fail_errno', source)
        self.assertIn('read(fd, buf, len)', source)
        self.assertIn('lseek(fd, original, SEEK_SET)', source)

if __name__ == '__main__': unittest.main()
