#!/usr/bin/env python3
"""Offline, source-only checks that never pretend to exercise Switch 2 hardware."""
from pathlib import Path
import re, unittest
R = Path(__file__).resolve().parents[1]

class SourceContract(unittest.TestCase):
    def test_pinned_core(self):
        self.assertEqual((R/'upstream-pinned/SOURCE_PIN.txt').read_text().strip(),
                         '0fc1b6c115519551031c51a20c9dc598fb1ea57d')

    def test_no_foreign_code(self):
        for p in (R/'native').glob('*'):
            if p.is_file():
                s = p.read_text().lower()
                self.assertNotIn('moonlightplatform', s)
                self.assertNotIn('gamestream', s)

    def test_switch2_platform_gate(self):
        for n in ('pf2_decoder_switch2.c', 'pf2_audio_switch2.c'):
            text = (R/'native'/n).read_text()
            self.assertIn('#if !defined(__SWITCH2__)', text)
            self.assertIn('#include <switch.h>', text)

    def test_decoder_capabilities(self):
        s = (R/'native/pf2_decoder_switch2.c').read_text()
        self.assertRegex(s, r'ViddecCodec_Av1')
        self.assertRegex(s, r'ViddecCodec_H264')
        self.assertNotIn('ViddecCodec_Hevc', s)
        self.assertIn('videoDecoderCopyFrame', s)
        self.assertIn('videoDecoderReleaseFrame', s)

if __name__ == '__main__': unittest.main()
