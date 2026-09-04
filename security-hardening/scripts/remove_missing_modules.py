#!/usr/bin/env python3
"""Remove ordinary Rust `mod name;` declarations whose source module no longer exists.

Only production `src/` trees are scanned. Integration-test files are crate roots and resolve
`mod common;` relative to their containing `tests/` directory, so they are deliberately excluded.
"""
from __future__ import annotations
import os,re,sys
from pathlib import Path

BLOCK=re.compile(
    r'(?P<block>(?:^[ \t]*#\[[^\n]+\]\n)*^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+(?P<name>[A-Za-z_][A-Za-z0-9_]*)[ \t]*;[ \t]*\n?)',
    re.MULTILINE,
)

def module_base(src: Path) -> Path:
    # Cargo crate roots resolve submodules beside the root file, even when the root
    # has a nonstandard name such as daemon_main.rs.
    if src.name in ('lib.rs','main.rs','mod.rs','daemon_main.rs') or src.parent.name == 'bin':
        return src.parent
    return src.parent/src.stem

def exists_for(src: Path,name: str) -> bool:
    base=module_base(src)
    return (base/f'{name}.rs').exists() or (base/name/'mod.rs').exists()

def main() -> int:
    root=Path(sys.argv[1] if len(sys.argv)>1 else '.').resolve(); changed=0
    sources=list((root/'src').rglob('*.rs'))
    for crate_src in (root/'crates').glob('*/src'):
        if crate_src.is_dir(): sources.extend(crate_src.rglob('*.rs'))
    for src in sorted(sources):
        text=src.read_text(encoding='utf-8'); old=text
        def repl(m: re.Match[str]) -> str:
            block=m.group('block')
            if '#[path' in block:
                return block
            name=m.group('name')
            if exists_for(src,name):
                return block
            print(f'remove missing module: {src.relative_to(root)} -> mod {name}')
            return ''
        text=BLOCK.sub(repl,text)
        if text!=old:
            tmp=src.with_name(src.name+'.rmux-mod-cleanup-tmp'); tmp.write_text(text,encoding='utf-8'); os.replace(tmp,src); changed+=1
    print(f'ordinary-module cleanup changed_files={changed}'); return 0
if __name__=='__main__': raise SystemExit(main())
