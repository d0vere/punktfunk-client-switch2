#!/usr/bin/env python3
"""Guarded, repeatable integration into an isolated OFFICIAL pinned worktree.

Each tracked file must equal the original HEAD blob or the previously installed
exact bytes. We never edit a user's modified tree; rollback uses backups and
content hashes. New crate sources and native files are tracked as additions.
"""
import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
PIN = 'a332988f7509bd4c6fbde7252bd1e2b874019f45'

def digest(b): return hashlib.sha256(b).hexdigest()

def replace_one(s, old, new, path):
    count = s.count(old)
    if count != 1:
        raise ValueError(f'{path}: anchor mismatch for {old[:70]!r}, found {count}')
    return s.replace(old, new)

def patched(path, text):
    s = text
    if path == 'Cargo.toml':
        s = replace_one(s,
            'pf-nx-platform = { path = "crates/pf-nx-platform" }\n',
            'pf-nx-switch2-platform = { path = "crates/pf-nx-switch2-platform" }\n', path)
        s = replace_one(s,
            'pf-nx-sys = { path = "crates/pf-nx-sys" }',
            'pf-nx-sys = { path = "crates/pf-nx-sys", default-features = false }', path)
        s = replace_one(s,
            '    "crates/pf-nx-probe",\n',
            '    "crates/pf-nx-probe",\n    "crates/pf-nx-switch2-platform",\n', path)
        s = replace_one(s,
            'features = ["textlayout"] }',
            'features = ["textlayout", "embed-freetype"] }', path)
    elif path == 'Cargo.lock':
        s = replace_one(s,
            ' "pf-nx-platform",\n "pf-nx-sys",\n',
            ' "pf-nx-switch2-platform",\n "pf-nx-sys",\n', path)
        entry = '''[[package]]
name = "pf-nx-switch2-platform"
version = "1.1.0"
dependencies = [
 "anyhow",
 "pf-nx-app",
 "pf-nx-sys",
 "punktfunk-core",
 "skia-safe",
 "tracing",
]

'''
        s = replace_one(s, '[[package]]\nname = "pf-nx-sys"',
            entry+'[[package]]\nname = "pf-nx-sys"', path)
    elif path == 'src/main.rs':
        a = s.index('#[cfg(target_os = "horizon")]\nfn main() {')
        b = s.index('\n#[cfg(not(target_os = "horizon"))]', a)
        s = s[:a] + '''#[cfg(target_os = "horizon")]
fn main() {
    // Official Horizon runtime linker guards retained; old NVK/AV backend is not linked.
    pf_nx_sys::rt::link_guard();
    let platform = pf_nx_switch2_platform::platform().unwrap_or_else(|e| {
        eprintln!("Punktfunk Switch 2 native initialization failed: {e:#}");
        std::process::exit(1)
    });
    // This is the ORIGINAL full frontend, discovery, pairing and QUIC session.
    pf_nx_app::run(platform)
}
''' + s[b:]
    elif path == 'crates/pf-nx-sys/build.rs':
        # A build-script-only cfg scopes the exclusion to the actual Switch2
        # build; the original Switch1 sys crate still exposes framebuffer/vi.
        s = replace_one(s,
            '    println!("cargo:rustc-check-cfg=cfg(pf_av)");',
            '''    println!("cargo:rustc-check-cfg=cfg(pf_av)");
    println!("cargo:rustc-check-cfg=cfg(pf2_switch2)");
    println!("cargo:rerun-if-env-changed=PF2_SWITCH2");''', path)
        s = replace_one(s,
            '    if os != "horizon" {\n        return;\n    }',
            '''    if os != "horizon" { return; }
    // libounce implements default_window.c on Nintendo Switch 2.
    // Suppress the legacy libnx (Switch 1) VI implementation only here.
    if env::var_os("PF2_SWITCH2").is_some() {
        println!("cargo:rustc-cfg=pf2_switch2");
        return;
    }''', path)
    elif path == 'crates/pf-nx-sys/src/lib.rs':
        # The legacy framebuffer depends on vi::default_window(), so both
        # modules must be absent together in the Switch 2 sys crate.
        s = replace_one(s, 'pub mod fb;\n',
                        '#[cfg(not(pf2_switch2))]\npub mod fb;\n', path)
        s = replace_one(s, 'pub mod vi;\n',
                        '#[cfg(not(pf2_switch2))]\npub mod vi;\n', path)
    elif path == '.cargo/config.toml':
        s = s.replace('/opt/devkitpro/libnx/include', '/opt/devkitpro/libounce/include')
        if '/opt/devkitpro/libnx/include' in s: raise ValueError('Old include directory remains')
    elif path == 'crates/pf-nx-app/src/runtime/menu.rs':
        # Only the native Switch2 port skips the synchronous all-tabs warm-up.
        # The user's first hardware log measured 50,961ms inside this call
        # before the first regular menu frame could be drawn.
        s = replace_one(s,
            '''    let warm = Instant::now();
    app.display.frame(&mut |canvas| {
        // One flush for the whole tour; flush per frame if the console's memory peaks there.
        console.warm_up(canvas, &viewport, |_| {});
    })?;
    tracing::info!("console: warmed the tabs in {} ms", warm.elapsed().as_millis());''',
            '''    if app.device.platform == "switch" && app.device.model == "Nintendo Switch 2" {
        tracing::info!("console: skip synchronous all-tabs warm-up on Switch 2; render menu now");
    } else {
        let warm = Instant::now();
        app.display.frame(&mut |canvas| {
            // Keep the original behavior on Switch 1 and on the dev box.
            console.warm_up(canvas, &viewport, |_| {});
        })?;
        tracing::info!("console: warmed the tabs in {} ms", warm.elapsed().as_millis());
    }''', path)
    elif path == 'crates/pf-nx-app/src/core/model.rs':
        s = replace_one(s, '    H264,\n    Hevc,\n}', '    H264,\n    Hevc,\n    Av1,\n}', path)
    elif path == 'crates/pf-nx-app/src/core/caps.rs':
        s = replace_one(s,
            '    pub h264: bool,\n    pub hevc: bool,',
            '    pub h264: bool,\n    pub hevc: bool,\n    pub av1: bool,', path)
        s = replace_one(s,
            '        h264: true,\n        hevc: true,',
            '        h264: true,\n        hevc: true,\n        av1: false,', path)
        s = replace_one(s,
            '        if self.h264 || self.hevc {',
            '        if self.h264 || self.hevc || self.av1 {', path)
        s = replace_one(s,
            '        if self.hevc {\n            prefs.push(CodecPref::Hevc);\n        }',
            '        if self.hevc {\n            prefs.push(CodecPref::Hevc);\n        }\n        if self.av1 {\n            prefs.push(CodecPref::Av1);\n        }', path)
    elif path == 'crates/pf-nx-app/src/session/connect.rs':
        s = replace_one(s,
            '        CodecPref::Hevc => quic::CODEC_HEVC,',
            '        CodecPref::Hevc => quic::CODEC_HEVC,\n        CodecPref::Av1 => quic::CODEC_AV1,', path)
    elif path == 'crates/pf-nx-app/src/core/settings.rs':
        s = replace_one(s,
            '        CodecPref::Hevc => "hevc",',
            '        CodecPref::Hevc => "hevc",\n        CodecPref::Av1 => "av1",', path)
        s = replace_one(s,
            '        "hevc" => CodecPref::Hevc,',
            '        "hevc" => CodecPref::Hevc,\n        "av1" => CodecPref::Av1,', path)
    else: raise ValueError('No patch defined for '+path)
    return s

CHANGES = ['Cargo.toml', 'Cargo.lock', 'src/main.rs', '.cargo/config.toml',
           'crates/pf-nx-sys/build.rs',
           'crates/pf-nx-sys/src/lib.rs',
           'crates/pf-nx-app/src/runtime/menu.rs',
           'crates/pf-nx-app/src/core/model.rs',
           'crates/pf-nx-app/src/core/caps.rs',
           'crates/pf-nx-app/src/session/connect.rs',
           'crates/pf-nx-app/src/core/settings.rs']

def call(*args, cwd):
    return subprocess.run(args, cwd=cwd, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True).stdout.strip()

def integrate(work):
    work = work.resolve(); repo=work/'official-client'
    if call('git', 'rev-parse', 'HEAD', cwd=repo) != PIN:
        raise ValueError('Must stage exact official commit first')
    state_file=work/'.pf2_integration.json'
    previous=json.loads(state_file.read_text()) if state_file.is_file() else {}
    outputs={}; backups={}
    for rel in CHANGES:
        f=repo/rel
        original=subprocess.run(['git','show','HEAD:'+rel],cwd=repo,check=True,stdout=subprocess.PIPE).stdout
        expected=patched(rel,original.decode()).encode()
        actual=f.read_bytes()
        if actual != expected and actual != original:
            raise ValueError(f'User-modified tracked file: {rel}. No changes applied.')
        outputs[rel]=expected
        backups[rel]=original
    crate=HERE/'rust/crates/pf-nx-switch2-platform'
    added={}
    for src in sorted(crate.rglob('*')):
        if not src.is_file(): continue
        relative='crates/pf-nx-switch2-platform/'+str(src.relative_to(crate))
        dst=repo/relative
        content=src.read_bytes()
        if dst.exists() and dst.read_bytes()!=content:
            raise ValueError(f'User-modified crate source: {relative}')
        added[relative]=content
    for rel, content in {**outputs, **added}.items():
        dest=repo/rel; dest.parent.mkdir(parents=True,exist_ok=True)
        if not dest.exists() or dest.read_bytes()!=content: dest.write_bytes(content)
    state={rel:{'original':digest(backups[rel]),'installed':digest(outputs[rel])} for rel in CHANGES}
    state.update({rel:{'original':None,'installed':digest(data)} for rel,data in added.items()})
    state_file.write_text(json.dumps(state,indent=2,sort_keys=True)+'\n')
    for f in added: call('git','add','-N','--',f,cwd=repo)
    diff=call('git','diff','--no-ext-diff','--binary',cwd=repo)
    (work/'official-integrated.diff').write_text(diff+'\n')
    print('Official frontend integration installed:',len(outputs),'tracked files +',len(added),'new crate files')
    print('Patch:',work/'official-integrated.diff')

if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--work',default=str(HERE/'work'))
    integrate(Path(parser.parse_args().work))
