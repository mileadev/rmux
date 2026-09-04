#!/usr/bin/env python3
"""Prune dormant remote-feature and Windows-only trees from the macOS fork."""
from __future__ import annotations
import os, re, shutil, sys
from pathlib import Path


def atomic_write(p: Path, s: str) -> None:
    q = p.with_name(p.name + '.rmux-stage15-tmp')
    q.write_text(s, encoding='utf-8')
    os.replace(q, p)


def edit(p: Path, fn) -> None:
    if not p.exists(): return
    o = p.read_text(encoding='utf-8')
    n = fn(o)
    if n != o:
        atomic_write(p, n)
        print(f'edited {p}')


def drop(p: Path) -> None:
    if not p.exists() and not p.is_symlink(): return
    if p.is_dir() and not p.is_symlink(): shutil.rmtree(p)
    else: p.unlink()
    print(f'deleted {p}')


def rx(t: str, p: str, r: str = '') -> str:
    return re.sub(p, r, t, flags=re.MULTILINE | re.DOTALL)


def remove_function(text: str, name: str) -> str:
    m = re.search(r'(?m)^[ \t]*(?:#\[[^\n]+\]\n[ \t]*)*fn[ \t]+' + re.escape(name) + r'\b[^\{]*\{', text)
    if not m: return text
    start = m.start()
    brace = text.find('{', m.start(), m.end())
    depth = 0
    i = brace
    in_str = None
    esc = False
    while i < len(text):
        c = text[i]
        if in_str:
            if esc: esc = False
            elif c == '\\': esc = True
            elif c == in_str: in_str = None
        else:
            if c in ('"', "'"):
                # Rust lifetime apostrophes are possible, but test bodies here do not
                # need lexical precision; braces inside quoted strings are ignored.
                if c == '"': in_str = c
            elif c == '{': depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    end = i + 1
                    while end < len(text) and text[end] in ' \t': end += 1
                    if end < len(text) and text[end] == '\n': end += 1
                    return text[:start] + text[end:]
        i += 1
    return text


def remove_matching_functions(text: str, name_pattern: str) -> str:
    while True:
        names = re.findall(r'(?m)^[ \t]*fn[ \t]+([A-Za-z_][A-Za-z0-9_]*)\b', text)
        target = next((n for n in names if re.search(name_pattern, n)), None)
        if target is None: return text
        new = remove_function(text, target)
        if new == text: return text
        text = new


def clean_manifest(t: str) -> str:
    # This fork is deliberately not buildable for Windows.
    t = rx(t, r"\n\[target\.'cfg\(windows\)'\.dependencies\]\n.*?(?=\n\[|\Z)", '\n')
    t = re.sub(r'(?m)^windows-sys\s*=.*\n', '', t)
    return t


def clean_daemon(t: str) -> str:
    t = re.sub(r'(?m)^const DEFAULT_WEB_PORT: u16 = 9777;\n', '', t)
    for field in ['web_frontend: Option<String>,', 'web_port: u16,', 'web_port_explicit: bool,', 'web_required: bool,']:
        t = t.replace('    ' + field + '\n', '')
    for init in ['web_frontend: None,', 'web_port: DEFAULT_WEB_PORT,', 'web_port_explicit: false,', 'web_required: false,']:
        t = t.replace('            ' + init + '\n', '')
    for name in ['web_port', 'web_port_explicit', 'web_required', 'web_frontend', 'with_web_port', 'with_web_frontend']:
        # public methods in the impl are brace-balanced and contain no nested impls.
        t = remove_function(t, name)
    t = rx(t, r'\n            \.with_web_options\(\n                self\.config\.web_port\(\),\n                self\.config\.web_frontend\(\)\.map\(str::to_owned\),\n                self\.config\.web_required\(\),\n                self\.config\.web_port_explicit\(\),\n            \)', '')
    return t


def clean_listener_options(t: str) -> str:
    for field in ['web_frontend: Option<String>,', 'web_port: u16,', 'web_port_explicit: bool,', 'web_required: bool,']:
        t = t.replace('    pub(crate) ' + field + '\n', '')
    for init in ['web_frontend: None,', 'web_port: 9777,', 'web_port_explicit: false,', 'web_required: false,']:
        t = t.replace('            ' + init + '\n', '')
    t = remove_function(t, 'with_web_options')
    return t


def clean_auto_start(t: str) -> str:
    for field in ['web_frontend: Option<String>,', 'web_port: Option<u16>,', 'web_required: bool,']:
        t = t.replace('    ' + field + '\n', '')
    for init in ['web_frontend: None,', 'web_port: None,', 'web_required: false,']:
        t = t.replace('            ' + init + '\n', '')
    for name in ['with_web_port', 'with_web_frontend', 'with_web_required']:
        t = remove_function(t, name)
    t = rx(t, r'\n        if let Some\(port\) = self\.web_port \{\n            command\.arg\("--web-port"\)\.arg\(port\.to_string\(\)\);\n        \}\n        if let Some\(frontend\) = &self\.web_frontend \{\n            command\.arg\("--frontend-url"\)\.arg\(frontend\);\n        \}', '')
    t = t.replace('a real tmux server socket.\n/// On Windows the client uses', 'a real tmux server socket.\n/// Legacy Windows builds used')
    return t


def clean_cli(t: str) -> str:
    t = t.replace('\\n  claude [install-skill|claude-args...]', '')
    t = t.replace('\\n  web-share [flags]\\n  web-share list|lookup|stop|disconnect|off|config', '')
    t = remove_matching_functions(t, r'claude|web_share')
    # Remove web-share-specific assertions from the mixed server-inventory test.
    t = rx(t, r'\n        let web_create = parse_cli\(\["rmux", "web-share", "-t", "alpha"\]\).*?\n        \}\n(?=    \})', '\n')
    return t


def clean_server_lifecycle(t: str) -> str:
    return remove_matching_functions(t, r'(^web_share_)|(^start_server_accepts_web_listener_flags$)')


def clean_surface_docs(t: str) -> str:
    return rx(t, r'\n        \(\n            &\["web-share", "--frontend-url"\]\[\.\.\],\n            "command web-share: --frontend-url expects an argument",\n        \),', '')


def clean_inventory_signatures(t: str) -> str:
    return rx(t, r'\n    \(\n        "web-share",\n        "\[-lX\].*?\n    \),', '')


def clean_inventory_tests(t: str) -> str:
    t = rx(t, r'\n        assert_eq!\(\n            resolve_list_commands_target\("web-share"\).*?\n        \);', '')
    t = remove_function(t, 'bare_listing_hides_extensions_while_explicit_lookup_keeps_them')
    return t


def clean_wire_ledger(t: str) -> str:
    t = t.replace('    UnsubscribePaneStreamRequest, UnsubscribePaneStreamResponse, WebShareConfigRequest,\n    WebShareListener, WebShareRequest, WebShareResponse, WindowTarget, RMUX_FRAME_MAGIC,\n',
                  '    UnsubscribePaneStreamRequest, UnsubscribePaneStreamResponse, WindowTarget, RMUX_FRAME_MAGIC,\n')
    t = re.sub(r'(?m)^\s*Request::WebShare\(Box::new\(WebShareRequest::Config\(WebShareConfigRequest\)\)\),\n', '', t)
    t = rx(t, r'\n        Response::WebShare\(Box::new\(WebShareResponse::Config\(.*?\n        \)\)\),', '')
    return t


def clean_live_compile(t: str) -> str:
    # Stage13 intentionally removed the platform parameter; finish two exact call sites.
    t = t.replace('                    bytes.as_ref(),\n                    windows_console_key.take(),\n                    active_emit_cache,',
                  '                    bytes.as_ref(),\n                    active_emit_cache,')
    # Read-only navigation has no platform key override in this fork.
    t = rx(t, r'\n            #\[cfg\(windows\)\]\n            let windows_key_override = windows_console_key\.and_then\(.*?\n            #\[cfg\(not\(windows\)\]\]\n            let windows_key_override = None;', '\n            let windows_key_override = None;')
    return t


def clean_sync_compile(t: str) -> str:
    t = t.replace('            AttachedPaneForward::EncodedKey(_) => {', '            AttachedPaneForward::EncodedKey => {')
    return t


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()

    # Dormant implementation/API trees forbidden by the local-only threat model.
    for rel in [
        'src/cli/web_share_display',
        'crates/rmux-sdk/src/web_share',
        'crates/rmux-server/src/handler_web_tests.rs',
        'crates/rmux-server/src/handler_web_stream.rs',
        'crates/rmux-client/src/attach_windows',
        'crates/rmux-pty/src/backend/windows',
        'crates/rmux-pty/tests/windows_conpty',
        'crates/rmux-ipc/tests/named_pipe_integration.rs',
    ]:
        drop(root / rel)

    for manifest in [root/'Cargo.toml', *sorted((root/'crates').glob('*/Cargo.toml'))]:
        edit(manifest, clean_manifest)

    edit(root/'crates/rmux-server/src/daemon.rs', clean_daemon)
    edit(root/'crates/rmux-server/src/listener_options.rs', clean_listener_options)
    edit(root/'crates/rmux-client/src/auto_start.rs', clean_auto_start)
    edit(root/'crates/rmux-server/src/handler_dispatch.rs', lambda t: t.replace('            | Request::WebShare(_)\n', ''))
    edit(root/'crates/rmux-core/src/command_inventory/signatures.rs', clean_inventory_signatures)
    edit(root/'crates/rmux-core/src/command_inventory.rs', clean_inventory_tests)
    edit(root/'src/cli.rs', clean_cli)
    edit(root/'src/cli_args_tests/server_lifecycle.rs', clean_server_lifecycle)
    edit(root/'src/cli_args_tests/surface_docs.rs', clean_surface_docs)
    edit(root/'crates/rmux-proto/tests/wire_ledger_v1.rs', clean_wire_ledger)
    edit(root/'crates/rmux-server/tests/request_end_to_end.rs', lambda t: t.replace('        "web-share",\n', ''))
    edit(root/'crates/rmux-core/src/dec_modes.rs', lambda t: t.replace('WebShare', 'detached clients').replace('web-share', 'detached client'))
    edit(root/'crates/rmux-server/src/server_runtime.rs', lambda t: t.replace('web-share tasks', 'background tasks').replace('web-share task', 'background task'))
    edit(root/'crates/rmux-server/src/handler_pane/attached_input/live.rs', clean_live_compile)
    edit(root/'crates/rmux-server/src/handler_pane/attached_input/synchronized.rs', clean_sync_compile)

    # The hidden daemon tests still encoded removed browser flags; production parser is already local-only.
    edit(root/'src/daemon_main.rs', lambda t: rx(t, r'\n#\[cfg\(test\)\]\nmod tests \{.*\n\}\s*$', '\n'))

    print('stage15 dormant remote/platform tree prune complete')
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
