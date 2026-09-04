#!/usr/bin/env python3
"""Remove remaining Windows-only pane forwarding and tests."""
from __future__ import annotations
import os, re, sys
from pathlib import Path

def write(p: Path, s: str) -> None:
    q=p.with_name(p.name+'.rmux-stage14-tmp'); q.write_text(s,encoding='utf-8'); os.replace(q,p)
def edit(p: Path, fn) -> None:
    if not p.exists(): return
    o=p.read_text(encoding='utf-8'); n=fn(o)
    if n!=o: write(p,n); print(f'edited {p}')
def rx(t,p,r=''): return re.sub(p,r,t,flags=re.MULTILINE|re.DOTALL)

def attached_input(t: str) -> str:
    t=t.replace('use std::marker::PhantomData;\n','')
    t=rx(t,r'#\[cfg\(windows\)\]\nuse rmux_pty::WindowsConsoleKeyEvent;\n','')
    t=rx(t,r'#\[derive\(Clone, Copy\)\]\nenum AttachedPaneForward<\'a> \{\n    EncodedKey\(PhantomData<&\'a \(\)>\),\n    #\[cfg\(windows\)\]\n    WindowsConsoleKey \{\n        key: WindowsConsoleKeyEvent,\n        bytes: &\'a \[u8\],\n    \},\n\}',
         '#[derive(Clone, Copy)]\nenum AttachedPaneForward {\n    EncodedKey,\n}')
    t=t.replace('AttachedPaneForward::EncodedKey(PhantomData)','AttachedPaneForward::EncodedKey')
    t=t.replace("forward: AttachedPaneForward<'_>,","forward: AttachedPaneForward,")
    t=t.replace('        // (issue #92); on Unix a\n        // real terminal paste into an ?2004h attach hits the same class, so\n        // the strip runs on every platform. Scrub the CONCATENATED buffer so\n',
                '        // Real terminal paste into an ?2004h attach hits the same class, so\n        // scrub the CONCATENATED buffer so\n')
    return t

def synchronized(t: str) -> str:
    t=rx(t,r'#\[cfg\(windows\)\]\nuse super::super::pane_io_encoding::\{.*?\n\};\n','')
    t=rx(t,r'\n    #\[cfg\(windows\)\]\n    WindowsConsoleKey \{\n        write: PaneConsoleInputWrite,\n        action: WindowsConsoleInputAction,\n    \},','')
    t=t.replace("forward: AttachedPaneForward<'_>,","forward: AttachedPaneForward,")
    t=rx(t,r'\n            #\[cfg\(windows\)\]\n            AttachedPaneForward::WindowsConsoleKey \{.*?\n            \}\n','\n')
    t=rx(t,r'\n            #\[cfg\(windows\)\]\n            PreparedAttachedPaneForward::WindowsConsoleKey \{ write, action \} => \{\n                write_attached_windows_console_input_action_to_target_io\(write, action\)\.await\?;\n            \}\n','\n')
    return t

def live(t: str) -> str:
    # Everything after the macOS live-key decode tests is Windows-only residue.
    t=rx(t,r'\n#\[cfg\(windows\)\]\nfn key_matches_name\(.*?\n#\[cfg\(all\(test, windows\)\)\]\nmod windows_console_binding_tests \{.*\n\}\s*$', '\n')
    return t

def main()->int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    edit(root/'crates/rmux-server/src/handler_pane/attached_input.rs',attached_input)
    edit(root/'crates/rmux-server/src/handler_pane/attached_input/synchronized.rs',synchronized)
    edit(root/'crates/rmux-server/src/handler_pane/attached_input/live.rs',live)
    print('stage14 Windows pane-forward cleanup complete')
    return 0
if __name__=='__main__': raise SystemExit(main())
