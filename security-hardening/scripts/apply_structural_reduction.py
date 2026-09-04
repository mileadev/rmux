#!/usr/bin/env python3
from __future__ import annotations
import json, os, re, shutil, subprocess, sys
from datetime import datetime, timezone
from pathlib import Path
EXPECTED='dfd68c774ca0f4212139a21d37d09c90f75f8bd7'

def run(cmd,cwd):
    return subprocess.run(cmd,cwd=cwd,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,check=True).stdout.strip()
def atomic_write(p,text):
    t=p.with_name(p.name+'.rmux-hardening-tmp'); t.write_text(text,encoding='utf-8'); os.replace(t,p)
def rm(root,rel,removed):
    p=root/rel
    if not p.exists() and not p.is_symlink(): return
    if p.is_dir() and not p.is_symlink(): shutil.rmtree(p)
    else: p.unlink()
    removed.append(rel)
def remove_section(text,section):
    return re.sub(r'(?ms)^\['+re.escape(section)+r'\]\s*\n.*?(?=^\[|\Z)','',text)
def replace(path,fn,changes):
    if not path.exists(): return
    old=path.read_text(encoding='utf-8'); new=fn(old)
    if new!=old: atomic_write(path,new); changes.append(str(path))
def is_windows_platform_file(name: str) -> bool:
    n=name.lower()
    return n == 'windows.rs' or n.startswith('windows_') or n.endswith('_windows.rs') or n.endswith('-windows.rs')

def main():
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    head=run(['git','rev-parse','HEAD'],root)
    if not (root/'Cargo.toml').exists(): raise SystemExit('not rmux checkout')
    removed=[]; changes=[]
    for rel in ['crates/rmux-web-crypto','crates/rmux-server/src/web','crates/rmux-server/tunnels','web-frontend','resources/windows','resources/claude','docs/web-share.md']:
        rm(root,rel,removed)
    skip={'.git','target','security-hardening'}
    candidates=[]
    for p in root.rglob('*'):
        if any(x in skip for x in p.parts): continue
        if not p.is_file() and not p.is_symlink(): continue
        n=p.name.lower()
        if any(tok in n for tok in ('web_share','web-share','tunnel','claude','conpty','powershell')):
            candidates.append(p)
        elif is_windows_platform_file(n):
            candidates.append(p)
    for p in sorted(candidates,key=lambda q:len(q.parts),reverse=True):
        if p.exists() or p.is_symlink(): rm(root,p.relative_to(root).as_posix(),removed)
    def root_manifest(t):
        t=t.replace('    "crates/rmux-web-crypto",\n','')
        t=remove_section(t,'profile.release.package.rmux-web-crypto')
        t=t.replace('    "/resources/windows/rmux.exe.manifest",\n','').replace('    "/resources/claude/skills/rmux/SKILL.md",\n','')
        t=re.sub(r'(?m)^qrcode\s*=.*\n','',t)
        t=t.replace('default = ["web"]','default = []')
        t=re.sub(r'(?m)^web\s*=\s*\["rmux-server/web"\]\s*\n','',t)
        t=remove_section(t,'build-dependencies')
        t=remove_section(t,"target.'cfg(windows)'.dependencies")
        return t
    replace(root/'Cargo.toml',root_manifest,changes)
    def server_manifest(t):
        for name in ['base64','getrandom','hkdf','httparse','rmux-web-crypto','zeroize','serde','serde_json','sha1','sha2','subtle','toml']:
            t=re.sub(r'(?m)^'+re.escape(name)+r'\s*=.*\n','',t)
        t=t.replace('default = ["web"]','default = []')
        t=re.sub(r'(?ms)^web\s*=\s*\[.*?^\]\s*\n','',t)
        return t
    replace(root/'crates/rmux-server/Cargo.toml',server_manifest,changes)
    def sdk_manifest(t):
        t=t.replace('default = ["web"]','default = []')
        t=re.sub(r'(?m)^web\s*=\s*\[\]\s*\n','',t)
        t=remove_section(t,"target.'cfg(windows)'.dependencies")
        return t
    replace(root/'crates/rmux-sdk/Cargo.toml',sdk_manifest,changes)
    replace(root/'crates/rmux-client/Cargo.toml',lambda t: remove_section(t,"target.'cfg(windows)'.dependencies"),changes)
    if (root/'build.rs').exists():
        atomic_write(root/'build.rs','fn main() {\n    println!("cargo:rerun-if-changed=build.rs");\n}\n')
        changes.append('build.rs')
    atomic_write(root/'README.md',"""# RMUX macOS local-only\n\nSecurity-reduced macOS-only fork of Helvesec/rmux v0.10.0.\n\n- Local PTYs and local terminal multiplexing only.\n- Same-user Unix-domain IPC only.\n- No Web Share, browser terminal, TCP/UDP listener, HTTP/WebSocket service, tunnel provider, SSH reverse forwarding, Tailscale Funnel/Serve, telemetry, or Claude integration.\n- Remote/sharing functionality is intentionally unsupported and removed from the active source tree.\n\nSee `security-hardening/` for the security gates and validation contract.\n""")
    report={'timestamp_utc':datetime.now(timezone.utc).isoformat(),'baseline_expected':EXPECTED,'source_head_before_reduction':head,'removed_paths':sorted(set(removed)),'edited_files':changes}
    atomic_write(root/'STRUCTURAL-REDUCTION.json',json.dumps(report,indent=2,sort_keys=True)+'\n')
    print(json.dumps(report,indent=2))
if __name__=='__main__': main()
