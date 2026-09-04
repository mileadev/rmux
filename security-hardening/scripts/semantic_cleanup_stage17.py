#!/usr/bin/env python3
"""Remove orphan Web Share APIs/tests after their backing state was deleted."""
from __future__ import annotations
import os, re, sys
from pathlib import Path


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + '.rmux-stage17-tmp')
    tmp.write_text(text, encoding='utf-8')
    os.replace(tmp, path)


def edit(path: Path, fn) -> None:
    if not path.exists():
        return
    old = path.read_text(encoding='utf-8')
    new = fn(old)
    if new != old:
        atomic_write(path, new)
        print(f'edited {path}')


def function_span(text: str, name: str):
    pattern = re.compile(
        r'(?m)^(?P<prefix>(?:(?:[ \t]*///[^\n]*\n)|(?:[ \t]*#\[[^\n]+\]\n))*)'
        r'[ \t]*(?:pub(?:\([^\n)]*\))?[ \t]+)?(?:const[ \t]+)?(?:async[ \t]+)?fn[ \t]+'
        + re.escape(name) + r'\b[^\{]*\{'
    )
    m = pattern.search(text)
    if not m:
        return None
    start = m.start()
    brace = text.find('{', m.start(), m.end())
    depth = 0
    i = brace
    in_string = False
    escaped = False
    while i < len(text):
        c = text[i]
        if in_string:
            if escaped:
                escaped = False
            elif c == '\\':
                escaped = True
            elif c == '"':
                in_string = False
        else:
            if c == '"':
                in_string = True
            elif c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    end = i + 1
                    while end < len(text) and text[end] in ' \t':
                        end += 1
                    if end < len(text) and text[end] == '\n':
                        end += 1
                    return start, end
        i += 1
    return None


def remove_function(text: str, name: str) -> str:
    while True:
        span = function_span(text, name)
        if span is None:
            return text
        text = text[:span[0]] + text[span[1]:]


def remove_named_functions(text: str, names: list[str]) -> str:
    for name in names:
        text = remove_function(text, name)
    return text


def clean_auto_start(text: str) -> str:
    text = remove_named_functions(text, [
        'with_web_port', 'with_web_frontend', 'with_web_required'
    ])
    text = re.sub(
        r'(?m)^    if config\.web_required \{\n        return None;\n    \}\n',
        '',
        text,
    )
    return text


def clean_daemon(text: str) -> str:
    return remove_named_functions(text, [
        'web_port', 'web_port_explicit', 'web_required', 'web_frontend',
        'with_web_port', 'with_web_frontend'
    ])


def clean_listener_options(text: str) -> str:
    return remove_function(text, 'with_web_options')


def clean_upgrade_restart(text: str) -> str:
    return remove_function(text, 'web_required_does_not_replace_active_no_web_daemon')


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    edit(root/'crates/rmux-client/src/auto_start.rs', clean_auto_start)
    edit(root/'crates/rmux-server/src/daemon.rs', clean_daemon)
    edit(root/'crates/rmux-server/src/listener_options.rs', clean_listener_options)
    edit(root/'crates/rmux-client/src/auto_start/upgrade_restart.rs', clean_upgrade_restart)
    edit(root/'src/server_runtime.rs', lambda t: t.replace(
        'Pane readers, IPC handlers, attach forwarding, and web-share tasks all run on',
        'Pane readers, IPC handlers, attach forwarding, and background tasks all run on'
    ))
    edit(root/'crates/rmux-server/tests/request_end_to_end.rs', lambda t: re.sub(
        r'(?m)^\s*"web-share",\n', '', t
    ))
    print('stage17 orphan Web Share API cleanup complete')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
