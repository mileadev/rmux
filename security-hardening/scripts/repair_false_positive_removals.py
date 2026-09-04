#!/usr/bin/env python3
"""Repair local-only functionality that broad filename/module cleanup must not remove."""
from __future__ import annotations
import os,subprocess,sys
from pathlib import Path

BASE='dfd68c774ca0f4212139a21d37d09c90f75f8bd7'
FALSELY_REMOVED=[
    'crates/rmux-server/src/handler_scripting_tests/list_windows_all.rs',
    'crates/rmux-server/src/handler_scripting_tests/parsed_queue_windows_mouse.rs',
    'tests/conformance/list_windows_keys.txt',
]

def git_show(root: Path,path: str) -> bytes:
    return subprocess.check_output(['git','show',f'{BASE}:{path}'],cwd=root)

def atomic(path: Path,data: bytes) -> None:
    path.parent.mkdir(parents=True,exist_ok=True)
    tmp=path.with_name(path.name+'.rmux-repair-tmp'); tmp.write_bytes(data); os.replace(tmp,path)

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve(); changed=0
    for rel in FALSELY_REMOVED:
        p=root/rel
        if not p.exists():
            atomic(p,git_show(root,rel)); changed+=1; print(f'restored baseline local feature file: {rel}')

    # Restore the two local tmux window/mouse test modules ("windows" means tmux windows here).
    hs=root/'crates/rmux-server/src/handler_scripting_tests.rs'
    text=hs.read_text(encoding='utf-8'); old=text
    anchor='#[path = "handler_scripting_tests/parsed_queue_core.rs"]\nmod parsed_queue_core;'
    blocks='''#[path = "handler_scripting_tests/list_windows_all.rs"]\nmod list_windows_all;\n\n#[path = "handler_scripting_tests/parsed_queue_windows_mouse.rs"]\nmod parsed_queue_windows_mouse;\n\n'''
    if 'mod list_windows_all;' not in text and anchor in text:
        text=text.replace(anchor,blocks+anchor)
    if text!=old:
        atomic(hs,text.encode()); changed+=1; print('restored local window/mouse module declarations')

    # Integration tests are independent crate roots: restore their shared `common` module when
    # it existed in the pinned baseline and the shared module still exists locally.
    for tests_dir in sorted((root/'crates').glob('*/tests')):
        common_exists=(tests_dir/'common.rs').exists() or (tests_dir/'common/mod.rs').exists()
        if not common_exists: continue
        for p in sorted(tests_dir.glob('*.rs')):
            rel=p.relative_to(root).as_posix()
            try: baseline=git_show(root,rel).decode('utf-8')
            except subprocess.CalledProcessError: continue
            if 'mod common;' not in baseline: continue
            cur=p.read_text(encoding='utf-8')
            if 'mod common;' in cur: continue
            lines=cur.splitlines(keepends=True)
            idx=0
            while idx < len(lines) and (lines[idx].startswith('#![') or lines[idx].strip()==''):
                idx+=1
            lines.insert(idx,'mod common;\n\n')
            atomic(p,''.join(lines).encode()); changed+=1; print(f'restored integration-test common module: {rel}')

    daemon=root/'src/daemon_main.rs'
    if daemon.exists() and (root/'src/server_runtime.rs').exists():
        cur=daemon.read_text(encoding='utf-8')
        if 'mod server_runtime;' not in cur:
            marker='use rmux_server::{ConfigFileSelection, DaemonConfig, ServerDaemon};\n'
            if marker in cur:
                cur=cur.replace(marker,'mod server_runtime;\n\n'+marker)
                atomic(daemon,cur.encode()); changed+=1; print('restored daemon server_runtime module')

    print(f'false-positive repair changed_items={changed}'); return 0
if __name__=='__main__': raise SystemExit(main())
