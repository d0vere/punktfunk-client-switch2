"""Resume helper must preserve and reuse preceding Skia build directories."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

# resume_skia.sh exports these values deliberately for the *real* cross-build.
# The unit-test fixture must not inherit them: its expected paths point into a
# temporary tree, while the shell running the tests may have real SDK paths.
_RESUME_OVERRIDES = (
    'PF2_SKIA_SOURCE_DIR',
    'PF2_CARGO_TARGET_DIR',
    'PF2_CARGO_HOME',
    'PF2_SKIA_CC',
    'PF2_SKIA_CXX',
)


def fixture_env(**overrides):
    env = os.environ.copy()
    for key in _RESUME_OVERRIDES:
        env.pop(key, None)
    env.update(overrides)
    return env

class ResumeTests(unittest.TestCase):
    @staticmethod
    def arrange(root):
        new=root/'new'
        new.mkdir()
        script=new/'resume_skia.sh'
        script.write_bytes((ROOT/'resume_skia.sh').read_bytes())
        script.chmod(0o755)
        build=new/'build_full.sh'
        build.write_text('#!/bin/sh\nprintf "%s\\n" "$PF2_SKIA_SOURCE_DIR" "$PF2_CARGO_TARGET_DIR" "$PF2_CARGO_HOME" "$PF2_SKIA_CC" "$PF2_SKIA_CXX" > "$PF2_ARGS_LOG"\n')
        build.chmod(0o755)
        return new

    def test_reuse_does_not_clean_cached_build(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            new=self.arrange(root)
            old=root/'old'
            source=old/'work/skia-source/m153-0.101.2'
            source.mkdir(parents=True)
            (source/'BUILD.gn').write_text('')
            target=old/'work/official-client/target'
            target.mkdir(parents=True)
            marker=target/'sentinel-do-not-delete'
            marker.write_text('KEEP')
            tools=old/'tooling'
            tools.mkdir()
            for tool in ['skia-cc.sh','skia-cxx.sh']:
                file=tools/tool
                file.write_text('#!/bin/sh\nexit 0\n')
                file.chmod(0o755)
            integration=root/'integration'
            (integration/'work/cargo-home').mkdir(parents=True)
            env=fixture_env(PF2_PREVIOUS_SKIA=str(old), PF2_PREVIOUS_INTEGRATION=str(integration),
                            PF2_ARGS_LOG=str(root/'received.txt'))
            result=subprocess.run([str(new/'resume_skia.sh')],env=env,capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            args=(root/'received.txt').read_text().splitlines()
            self.assertEqual(args,[str(source),str(target),str(integration/'work/cargo-home'),
                                   str(tools/'skia-cc.sh'),str(tools/'skia-cxx.sh')])
            self.assertEqual(marker.read_text(),'KEEP')

    def test_missing_previous_build_refuses_cleanly(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            new=self.arrange(root)
            env=fixture_env(PF2_PREVIOUS_SKIA=str(root/'absent'))
            result=subprocess.run([str(new/'resume_skia.sh')],env=env,capture_output=True,text=True)
            self.assertEqual(result.returncode,27)
            self.assertIn('No verified previous Skia build',result.stderr)

if __name__=='__main__':
    unittest.main()
