#!/usr/bin/env python3
"""Immutable upstream checkout + non-destructive installation of independent native sources.

This stages source files, not a completed platform implementation; no release can be
made by the staging step alone. All Git operations occur under --work, never on the
user's original repository.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

COMMIT = 'a332988f7509bd4c6fbde7252bd1e2b874019f45'
CORE = '0fc1b6c115519551031c51a20c9dc598fb1ea57d'
UPSTREAM = 'https://github.com/punktfunk/client-switch.git'
HERE = Path(__file__).resolve().parents[1]
NATIVE = HERE / 'native'

def call(*args, cwd=None):
    p = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE)
    if p.returncode:
        raise RuntimeError(f"{' '.join(map(str,args))}: {p.stderr.strip()}")
    return p.stdout.strip()

def sha256(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()

def stage(root: Path, source: str):
    root = root.expanduser().resolve()
    # Only this workspace is written, never the repository next to it.
    root.mkdir(parents=True, exist_ok=True)
    repo = root / 'official-client'
    fresh = not repo.exists()
    if fresh:
        # A previously verified official worktree is a usable *local object mirror*;
        # never use its modified files as the source of the final code.
        previous = HERE.parent / 'Punktfunk_Official_Switch2_Source_Work' / 'work' / 'official-client'
        if source == UPSTREAM and (previous/'.git').exists():
            try:
                call('git','cat-file','-e',COMMIT+'^{commit}',cwd=previous)
                source = str(previous)
                print('Using previous official Git object mirror:',previous)
            except RuntimeError:
                pass
        call('git', 'clone', '--no-checkout', source, str(repo))
    if not (repo / '.git').exists():
        raise RuntimeError(f'{repo}: destination exists but is not a git clone')
    head_before = call('git', 'rev-parse', 'HEAD', cwd=repo)
    current_edits = call('git','status','--porcelain','--untracked-files=no',cwd=repo)
    installed_manifest = root/'.pf2_injected.json'
    installed_before = json.loads(installed_manifest.read_text()) if installed_manifest.is_file() else {}
    integrated_manifest = root/'.pf2_integration.json'
    integrated_before = json.loads(integrated_manifest.read_text()) if integrated_manifest.is_file() else {}
    if not fresh and current_edits:
        # Re-running the installer is safe only for this tool's own, unchanged
        # intent-to-add overlay. Tracked upstream modifications always block it.
        for line in current_edits.splitlines():
            status, rel = line[:2], line[3:]
            expected = installed_before.get(rel)
            path = repo/rel
            if status == ' M' and rel in integrated_before and path.is_file() and \
                    sha256(path) == integrated_before[rel]['installed']:
                continue
            # Intent-to-add Rust crate files belong to the own integration overlay too.
            # A genuine user change to any of them still blocks staging.
            if status == ' A' and path.is_file():
                if expected and sha256(path) == expected:
                    continue
                integration = integrated_before.get(rel, {})
                if integration.get('original') is None and integration.get('installed') == sha256(path):
                    continue
            raise RuntimeError(f'Existing checkout has user edits: {rel}')
    if head_before != COMMIT or fresh:
        if current_edits and not fresh:
            raise RuntimeError('Non-destructive checkout refused due to pending overlay')
        try:
            call('git', 'cat-file', '-e', COMMIT+'^{commit}', cwd=repo)
        except RuntimeError:
            call('git', 'fetch', '--no-tags', 'origin', COMMIT, cwd=repo)
        call('git', 'checkout', '--detach', COMMIT, cwd=repo)
    head=call('git','rev-parse','HEAD',cwd=repo)
    if head != COMMIT:
        raise RuntimeError('Invalid official source commit: '+head)
    lock = repo/'Cargo.lock'
    lock_blob = call('git','rev-parse','HEAD:Cargo.lock',cwd=repo)
    original_lock = subprocess.run(['git','show','HEAD:Cargo.lock'],cwd=repo,check=True,stdout=subprocess.PIPE).stdout
    if sha256(lock) != hashlib.sha256(original_lock).hexdigest():
        if 'Cargo.lock' not in integrated_before or sha256(lock) != integrated_before['Cargo.lock']['installed']:
            raise RuntimeError('Cargo.lock changed outside guarded integration')
    expected_lock_blob = 'e8c9787c5017d82dc4822a4236506b7c7382d0d4'
    if lock_blob != expected_lock_blob:
        raise RuntimeError(f'Unexpected Cargo.lock git blob {lock_blob} != {expected_lock_blob}')
    manifest=(repo/'Cargo.toml').read_text()
    if CORE not in manifest:
        raise RuntimeError('Core pin is not present in root Cargo.toml')
    dest = repo/'ports'/'switch2'/'native'
    dest.mkdir(parents=True,exist_ok=True)
    installed={}
    for src in sorted(NATIVE.glob('*')):
        if not src.is_file():continue
        target=dest/src.name
        if target.exists() and sha256(target)!=sha256(src):
            raise RuntimeError(f'Would overwrite a different source: {target}')
        if not target.exists(): target.write_bytes(src.read_bytes())
        installed[str(target.relative_to(repo))]=sha256(src)
    # Intent-to-add makes git diff include new files without committing or staging their bytes.
    call('git','add','-N','--',*installed.keys(),cwd=repo)
    diff = call('git','diff','--no-ext-diff','--binary','--',*installed.keys(),cwd=repo)
    (root/'official-overlay.diff').write_text(diff+'\n')
    report={'source_repository':UPSTREAM,'commit':head,'core_pin':CORE,
            'cargo_lock_blob':lock_blob,'cargo_lock_sha256':sha256(lock),
            'new_native_files':installed,
            'full_frontend_integration':False,'nro_eligible':False}
    (root/'staging_report.json').write_text(json.dumps(report,indent=2,sort_keys=True)+'\n')
    (root/'.pf2_injected.json').write_text(json.dumps(installed,indent=2,sort_keys=True)+'\n')
    print('Staging upstream verified:',head)
    print('Pinned Cargo.lock:',sha256(lock))
    print('Native sources overlay:',len(installed))
    print('Diff:',root/'official-overlay.diff')
    return repo

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--work',default=os.environ.get('PF2_WORK',str(HERE/'work')))
    ap.add_argument('--source',default=os.environ.get('PF2_UPSTREAM',UPSTREAM))
    a=ap.parse_args()
    try: stage(Path(a.work),a.source)
    except Exception as e: print('STAGING FAILURE:',e,file=sys.stderr);sys.exit(1)

if __name__=='__main__': main()
