#!/usr/bin/env python3
"""Remove Rust #[path = "..."] module declarations whose target file was deleted.

The hardened fork deliberately deletes complete Windows/Web/Tunnel implementation files.
rustfmt still resolves path-attributed modules even when cfg-disabled, so stale declarations
must also disappear. This pass is idempotent and only removes declarations whose concrete
path target does not exist.
"""
from __future__ import annotations
import os,re,sys
from pathlib import Path

BLOCK = re.compile(
    r'(?P<block>(?:^[ \t]*#\[[^\n]+\]\n)*^[ \t]*#\[path\s*=\s*"(?P<path>[^"]+)"\]\n^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+[A-Za-z_][A-Za-z0-9_]*[ \t]*;[ \t]*\n?)',
    re.MULTILINE,
)

def resolve_target(source: Path, rel: str) -> Path:
    return (source.parent / rel).resolve()

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve()
    changed=0
    for src in sorted(list((root/'src').rglob('*.rs')) + list((root/'crates').rglob('*.rs'))):
        text=src.read_text(encoding='utf-8'); old=text
        while True:
            removed=False
            def repl(m: re.Match[str]) -> str:
                nonlocal removed
                target=resolve_target(src,m.group('path'))
                if not target.exists():
                    print(f'remove missing module: {src.relative_to(root)} -> {m.group("path")}')
                    removed=True
                    return ''
                return m.group('block')
            text2=BLOCK.sub(repl,text)
            text=text2
            if not removed: break
        if text!=old:
            tmp=src.with_name(src.name+'.rmux-path-cleanup-tmp')
            tmp.write_text(text,encoding='utf-8'); os.replace(tmp,src); changed+=1
    print(f'missing-path cleanup changed_files={changed}')
    return 0
if __name__=='__main__': raise SystemExit(main())
