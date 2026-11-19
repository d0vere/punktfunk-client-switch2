#!/usr/bin/env python3
"""Remove only still-unchanged files our staging step installed; no git reset."""
import argparse,hashlib,json
from pathlib import Path

def main():
    p=argparse.ArgumentParser();p.add_argument('--work',default='work');a=p.parse_args()
    root=Path(a.work).expanduser().resolve(); metadata=root/'.pf2_injected.json'
    if not metadata.is_file(): print('No staging metadata; nothing to undo');return
    repo=root/'official-client'
    installed=json.loads(metadata.read_text()); remaining={}
    for rel, expected in installed.items():
        target=(repo/rel).resolve()
        if not target.is_relative_to(repo.resolve()):raise RuntimeError('Unsafe tracked path')
        if not target.exists():continue
        actual=hashlib.sha256(target.read_bytes()).hexdigest()
        if actual!=expected:
            print('PRESERVED (modified):',target)
            remaining[rel]=expected
        else:
            target.unlink();print('Removed:',target)
    if remaining:metadata.write_text(json.dumps(remaining,indent=2)+'\n')
    else: metadata.unlink()
    # The upstream git history, the lockfile and any existing user data remain untouched.
if __name__=='__main__':main()
