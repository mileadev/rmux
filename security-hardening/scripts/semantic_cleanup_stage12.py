#!/usr/bin/env python3
"""Remove the remaining Windows-specific attach-stream wire surface."""
from __future__ import annotations
import os, re, sys
from pathlib import Path


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + '.rmux-stage12-tmp')
    tmp.write_text(text, encoding='utf-8')
    os.replace(tmp, path)


def edit(path: Path, fn) -> None:
    if not path.exists(): return
    old = path.read_text(encoding='utf-8')
    new = fn(old)
    if new != old:
        atomic_write(path, new)
        print(f'edited {path}')


def rx(t: str, p: str, r: str = '') -> str:
    return re.sub(p, r, t, flags=re.MULTILINE | re.DOTALL)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()

    def lib_rs(t: str) -> str:
        t = t.replace('    AttachMessage, AttachShellCommand, AttachedKeystroke, AttachedWindowsConsoleKey, KeyDispatched,\n',
                      '    AttachMessage, AttachShellCommand, AttachedKeystroke, KeyDispatched,\n')
        t = t.replace('    CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY, ', '')
        t = t.replace('    CAPABILITY_ATTACH_WINDOWS_CONSOLE_KEY,\n', '')
        return t
    edit(root/'crates/rmux-proto/src/lib.rs', lib_rs)

    def attach(t: str) -> str:
        t = re.sub(r'(?m)^const WINDOWS_CONSOLE_KEYSTROKE_TAG: u8 = 14;\n', '', t)
        # Collapse AttachedKeystroke to bytes only.
        t = t.replace('pub struct AttachedKeystroke {\n    bytes: Vec<u8>,\n    windows_console_key: Option<AttachedWindowsConsoleKey>,\n}',
                      'pub struct AttachedKeystroke {\n    bytes: Vec<u8>,\n}')
        t = t.replace('        Self {\n            bytes,\n            windows_console_key: None,\n        }', '        Self { bytes }')
        t = rx(t, r'\n    /// Attaches the original Windows console key event that produced this byte sequence\..*?\n    pub fn with_windows_console_key.*?\n    \}\n', '\n')
        t = rx(t, r'\n    /// Returns the original Windows console key event when the client captured one\..*?\n    pub fn windows_console_key.*?\n    \}\n', '\n')
        t = rx(t, r'\n#\[derive\(Debug, Clone, PartialEq, Eq, Serialize, Deserialize\)\]\nstruct AttachedWindowsConsoleKeystroke.*?\n\}\n\nimpl AttachedWindowsConsoleKeystroke.*?\n\}\n', '\n')
        t = rx(t, r'\n/// Original Windows console key data for attach clients running on ConPTY\..*?\nimpl AttachedWindowsConsoleKey \{.*?\n\}\n', '\n')
        t = re.sub(r'(?m)^\s*WINDOWS_CONSOLE_KEYSTROKE_TAG => self\.next_windows_console_keystroke_message\(\),\n', '', t)
        t = rx(t, r'\n    fn next_windows_console_keystroke_message\(.*?\n    \}\n\n    fn next_key_dispatched_message',
               '\n    fn next_key_dispatched_message')
        t = rx(t, r'\nfn encode_keystroke_message\(keystroke: &AttachedKeystroke\) -> Result<Vec<u8>, RmuxError> \{.*?\n\}\n\nfn encode_resize_message',
               '\nfn encode_keystroke_message(keystroke: &AttachedKeystroke) -> Result<Vec<u8>, RmuxError> {\n    encode_structured_message(KEYSTROKE_TAG, &keystroke.bytes)\n}\n\nfn encode_resize_message')
        return t
    edit(root/'crates/rmux-proto/src/attach.rs', attach)

    def tests(t: str) -> str:
        t = t.replace('    AttachedKeystroke, KeyDispatched, KEYSTROKE_TAG, WINDOWS_CONSOLE_KEYSTROKE_TAG,\n',
                      '    AttachedKeystroke, KeyDispatched, KEYSTROKE_TAG,\n')
        t = t.replace('    AttachedWindowsConsoleKey, RmuxError, TerminalGeometry, TerminalPixels, TerminalSize,\n',
                      '    RmuxError, TerminalGeometry, TerminalPixels, TerminalSize,\n')
        t = rx(t, r'\n#\[test\]\nfn keystroke_messages_preserve_windows_console_key\(\) \{.*?\n\}\n', '\n')
        # Platform-neutral fixtures for local shell command codec tests.
        t = t.replace('"pwsh.exe".to_owned(),\n        "C:\\\\work".to_owned(),',
                      '"/bin/sh".to_owned(),\n        "/tmp".to_owned(),')
        t = t.replace('"C:\\\\Program Files\\\\PowerShell\\\\7\\\\pwsh.exe".to_owned(),\n        "C:\\\\repo".to_owned(),',
                      '"/bin/sh".to_owned(),\n        "/tmp".to_owned(),')
        return t
    edit(root/'crates/rmux-proto/src/attach/tests.rs', tests)

    print('stage12 Windows attach protocol cleanup complete')
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
