#!/usr/bin/env python3
"""Collapse server attached input to the macOS byte-only path."""
from __future__ import annotations
import os, re, sys
from pathlib import Path


def atomic_write(p: Path, s: str) -> None:
    q=p.with_name(p.name+'.rmux-stage13-tmp'); q.write_text(s,encoding='utf-8'); os.replace(q,p)
def edit(p: Path, fn) -> None:
    if not p.exists(): return
    o=p.read_text(encoding='utf-8'); n=fn(o)
    if n!=o: atomic_write(p,n); print(f'edited {p}')
def rx(t,p,r=''): return re.sub(p,r,t,flags=re.MULTILINE|re.DOTALL)


def clean_live(t: str) -> str:
    t=rx(t,r'#\[cfg\(windows\)\]\nuse rmux_core::\{.*?\};\n','')
    t=rx(t,r'#\[cfg\(windows\)\]\nuse rmux_pty::WindowsConsoleKeyEvent;\n','')
    t=t.replace('    windows_console_key: Option<rmux_proto::AttachedWindowsConsoleKey>,\n','')
    t=t.replace('    fn new(\n        bytes: Arc<[u8]>,\n        windows_console_key: Option<rmux_proto::AttachedWindowsConsoleKey>,\n    ) -> Self {','    fn new(bytes: Arc<[u8]>) -> Self {')
    t=t.replace('            windows_console_key,\n','')
    t=t.replace('            windows_console_key: None,\n','')

    # Windows-only attach entrypoints are gone; generic keystrokes are byte-only.
    t=rx(t,r'\n    #\[cfg\(all\(test, windows\)\)\]\n    pub\(crate\) async fn handle_attached_keystroke_input\(.*?\n    \}\n\n    #\[cfg\(all\(test, windows\)\)\]\n    pub\(crate\) async fn handle_attached_keystroke_input_for_identity\(.*?\n    \}\n','\n')
    t=rx(t,r'self\.handle_attached_live_input_inner_with_windows_console_key\(\n\s*identity,\n\s*pending_input,\n\s*keystroke\.bytes\(\),\n\s*keystroke\.windows_console_key\(\),\n\s*active_emit_cache,\n\s*\)\n\s*\.await',
         'self.handle_attached_live_input_inner_cached(\n            identity,\n            pending_input,\n            keystroke.bytes(),\n            active_emit_cache,\n        )\n        .await')

    # Keep a small wrapper and a single implementation without platform state.
    t=rx(t,r'self\.handle_attached_live_input_inner_with_windows_console_key\(\n\s*identity,\n\s*pending_input,\n\s*bytes,\n\s*None,\n\s*active_emit_cache,\n\s*\)\n\s*\.await',
         'self.handle_attached_live_input_inner_cached_impl(\n            identity, pending_input, bytes, active_emit_cache,\n        )\n        .await')
    t=t.replace('    async fn handle_attached_live_input_inner_with_windows_console_key(\n','    async fn handle_attached_live_input_inner_cached_impl(\n')
    t=t.replace('        windows_console_key: Option<rmux_proto::AttachedWindowsConsoleKey>,\n','')

    # All chunk/work-queue calls now carry bytes only.
    t=re.sub(r'(?m)^\s*windows_console_key,\n','',t)
    t=t.replace('AttachedLiveInputWork::new(Arc::from(bytes), None)','AttachedLiveInputWork::new(Arc::from(bytes))')
    t=t.replace('AttachedLiveInputWork::new(Arc::from(bytes), windows_console_key)','AttachedLiveInputWork::new(Arc::from(bytes))')
    t=rx(t,r'\n        if windows_console_key\.is_none\(\) \{\n            if let Some\(forwarded\) = self\n                \.try_forward_plain_attached_bytes_fast\((.*?)\n            \{\n                return Ok\(forwarded\);\n            \}\n        \}',
         r'\n        if let Some(forwarded) = self\n                .try_forward_plain_attached_bytes_fast(\1\n            {\n                return Ok(forwarded);\n            }')
    t=rx(t,r'\n            let windows_console_key = work\.windows_console_key\.take\(\);','')
    t=t.replace('        mut windows_console_key: Option<rmux_proto::AttachedWindowsConsoleKey>,\n','')
    t=t.replace('        windows_console_key: Option<rmux_proto::AttachedWindowsConsoleKey>,\n','')
    t=rx(t,r'\n        #\[cfg\(not\(windows\)\)\]\n        let _ = windows_console_key;\n        #\[cfg\(windows\)\]\n        let windows_console_key = windows_console_key\n            \.filter\(\|_\| pending_input\.is_empty\(\) && !bytes\.is_empty\(\)\)\n            \.map\(windows_console_key_event\);','')
    t=rx(t,r'\n        #\[cfg\(windows\)\]\n        let try_plain_fast_path = windows_console_key\.is_none\(\);\n        #\[cfg\(not\(windows\)\)\]\n        let try_plain_fast_path = true;','')
    t=t.replace('        if try_plain_fast_path {\n            if let Some(forwarded) = self','        if let Some(forwarded) = self')
    t=t.replace('                return Ok(AttachedLiveInputStep::Complete(forwarded));\n            }\n        }','                return Ok(AttachedLiveInputStep::Complete(forwarded));\n            }',1)

    # Read-only handling has no platform key override.
    t=rx(t,r'\n            #\[cfg\(windows\)\]\n            let windows_key_override = windows_console_key\.and_then\(.*?\n            #\[cfg\(not\(windows\)\]\n            let windows_key_override = None;', '\n            let windows_key_override = None;')
    t=t.replace('                // parse, this preserves the native Windows KEY_EVENT that\n                // belongs to these exact bytes.\n','                // parse, this preserves the exact input bytes for this step.\n')

    # Delete Windows-only dispatch branches; retain their former non-Windows branch directly.
    t=rx(t,r'\n        #\[cfg\(windows\)\]\n        if pending_input\.is_empty\(\) && bytes == b"\\x04" \{.*?\n        \}\n','\n')
    t=rx(t,r'\n        #\[cfg\(windows\)\]\n        if let Some\(key_event\) = windows_console_key\.filter\(.*?\n        \}\n','\n')
    t=rx(t,r'\n                        #\[cfg\(windows\)\]\n                        let handled = if let Some\(key_event\) = windows_console_key.*?\n                        #\[cfg\(not\(windows\)\)\]\n                        let handled = self\.handle_attached_live_key\(identity, key\)\.await\?;',
         '\n                        let handled = self.handle_attached_live_key(identity, key).await?;')
    t=rx(t,r'\n                    #\[cfg\(windows\)\]\n                    let handled = if let Some\(key_event\) = windows_console_key.*?\n                    #\[cfg\(not\(windows\)\)\]\n                    let handled = self\.handle_attached_live_key\(identity, key\)\.await\?;',
         '\n                    let handled = self.handle_attached_live_key(identity, key).await?;')

    # Any now-orphaned platform helper functions at file scope are removed conservatively by name.
    t=rx(t,r'\n#\[cfg\(windows\)\]\nfn windows_[A-Za-z0-9_]+\(.*?\n\}\n','\n')
    return t


def main()->int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    edit(root/'crates/rmux-server/src/handler_pane/attached_input/live.rs',clean_live)
    # Import is now used only by the test-only helper made in stage11.
    edit(root/'crates/rmux-server/src/handler_attach/registration.rs',lambda t:
         t.replace('use crate::client_names::attached_client_name;\n','#[cfg(test)]\nuse crate::client_names::attached_client_name;\n'))
    print('stage13 macOS attached-input cleanup complete')
    return 0
if __name__=='__main__': raise SystemExit(main())
