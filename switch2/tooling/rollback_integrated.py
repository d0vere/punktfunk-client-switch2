#!/usr/bin/env python3
"""Rollback only integration-owned identical bytes; preserve user modifications."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

p=argparse.ArgumentParser();p.add_argument('--work',default='work');args=p.parse_args()
work=Path(args.work).expanduser().resolve();root=work/'official-client';state=work/'.pf2_integration.json'
if not state.is_file(): print('No Rust integration to roll back');raise SystemExit(0)
entries=json.loads(state.read_text()); left={}
for name, record in sorted(entries.items(),key=lambda t:len(t[0]),reverse=True):
    path=(root/name).resolve()
    if not path.is_relative_to(root): raise RuntimeError('Refusing path outside worktree')
    if not path.exists():continue
    actual=hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != record['installed']:
        print('PRESERVED USER EDIT:',name);left[name]=record;continue
    if record['original']:
        raw=subprocess.run(['git','show','HEAD:'+name],cwd=root,check=True,stdout=subprocess.PIPE).stdout
        if hashlib.sha256(raw).hexdigest()!=record['original']:
            raise RuntimeError('Official source hash changed: '+name)
        path.write_bytes(raw)
        print('Restored official blob:',name)
    else:
        path.unlink();print('Removed new source:',name)
if left:state.write_text(json.dumps(left,indent=2,sort_keys=True)+'\n')
else:state.unlink()
