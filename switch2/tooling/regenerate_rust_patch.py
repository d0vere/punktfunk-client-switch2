#!/usr/bin/env python3
"""Generate only new Switch 2 Rust crate files as a reproducible Git patch.

Tracked upstream edits are applied by guarded tooling/apply_integration.py and
only appear in work/official-integrated.diff once an official clone is staged.
"""
from pathlib import Path
import difflib
import hashlib

ROOT=Path(__file__).resolve().parents[1]
CRATE=ROOT/'rust/crates/pf-nx-switch2-platform'
chunks=[]
for f in sorted(CRATE.rglob('*')):
    if not f.is_file(): continue
    rel='crates/pf-nx-switch2-platform/'+str(f.relative_to(CRATE))
    data=f.read_bytes()
    blob=hashlib.sha1(f'blob {len(data)}\0'.encode()+data).hexdigest()[:7]
    diff=''.join(difflib.unified_diff([], f.read_text().splitlines(keepends=True),
                                       fromfile='/dev/null',tofile='b/'+rel,lineterm='\n'))
    chunks.append(f'diff --git a/{rel} b/{rel}\nnew file mode 100644\nindex 0000000..{blob}\n'+diff)
(ROOT/'patches/rust-additions.patch').write_text(''.join(chunks))
print('Patch written: patches/rust-additions.patch')
