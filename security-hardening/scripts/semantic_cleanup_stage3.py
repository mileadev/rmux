#!/usr/bin/env python3
from __future__ import annotations
import os, sys
from pathlib import Path

def edit(path: Path, replacements: list[tuple[str,str]]) -> None:
    if not path.exists(): return
    old=path.read_text(encoding='utf-8'); new=old
    for a,b in replacements: new=new.replace(a,b)
    if new!=old:
        tmp=path.with_name(path.name+'.rmux-stage3-tmp'); tmp.write_text(new,encoding='utf-8'); os.replace(tmp,path); print(f'edited {path}')

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    edit(root/'crates/rmux-client/src/control.rs', [('\n#[cfg(test)]\n#[path = "control/windows_tests.rs"]\nmod windows_tests;','')])
    edit(root/'crates/rmux-server/src/handler_pane.rs', [
        ('#[cfg(windows)]\n#[path = "handler_pane/windows_console_sequence.rs"]\nmod pane_windows_console_sequence;\n',''),
    ])
    edit(root/'src/main.rs', [
        ('#[cfg(windows)]\nmod windows_shell;\n',''),
        ('#[cfg(windows)]\nmod windows_terminal;\n',''),
    ])
    print('stage3 cleanup complete'); return 0
if __name__=='__main__': raise SystemExit(main())
