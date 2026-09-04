#!/usr/bin/env python3
"""Remove stale module declarations created by the macOS local-only reduction.

Only declarations are removed when the referenced source file is absent AND the
module/attributes/path are in an intentionally removed feature/platform family.
This makes the pass conservative and idempotent.
"""
from __future__ import annotations
import os
import re
import sys
from pathlib import Path

FORBIDDEN = ("windows", "web", "tunnel", "claude", "conpty", "powershell")

ATTR_MOD = re.compile(
    r"(?ms)(?P<block>(?P<attrs>(?:^[ \t]*#\[[^\n]+\]\n)+)^[ \t]*(?P<vis>pub(?:\([^\n]*?\))?[ \t]+)?mod[ \t]+(?P<name>[A-Za-z_][A-Za-z0-9_]*)[ \t]*;[ \t]*\n?)"
)
BARE_MOD = re.compile(
    r"(?m)^(?P<indent>[ \t]*)(?P<vis>pub(?:\([^\n]*?\))?[ \t]+)?mod[ \t]+(?P<name>[A-Za-z_][A-Za-z0-9_]*)[ \t]*;[ \t]*\n?"
)
PATH_ATTR = re.compile(r"#\[path[ \t]*=[ \t]*\"([^\"]+)\"\]")


def module_base(src: Path) -> Path:
    if src.name in {"lib.rs", "main.rs", "mod.rs"}:
        return src.parent
    return src.parent / src.stem


def candidates(src: Path, name: str, attrs: str) -> list[Path]:
    match = PATH_ATTR.search(attrs)
    if match:
        return [src.parent / match.group(1)]
    base = module_base(src)
    return [base / f"{name}.rs", base / name / "mod.rs"]


def forbidden_scope(name: str, attrs: str) -> bool:
    hay = f"{name}\n{attrs}".lower()
    return any(token in hay for token in FORBIDDEN)


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + ".rmux-missing-mod-tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def clean_one(src: Path) -> int:
    original = src.read_text(encoding="utf-8")
    text = original
    removed = 0

    def repl_attr(match: re.Match[str]) -> str:
        nonlocal removed
        attrs = match.group("attrs")
        name = match.group("name")
        refs = candidates(src, name, attrs)
        if forbidden_scope(name, attrs) and not any(p.exists() for p in refs):
            print(f"remove stale module: {src}: {name} -> {[str(p) for p in refs]}")
            removed += 1
            return ""
        return match.group("block")

    text = ATTR_MOD.sub(repl_attr, text)

    # Catch bare forbidden modules after attributed declarations were processed.
    def repl_bare(match: re.Match[str]) -> str:
        nonlocal removed
        name = match.group("name")
        refs = candidates(src, name, "")
        if forbidden_scope(name, "") and not any(p.exists() for p in refs):
            print(f"remove stale bare module: {src}: {name} -> {[str(p) for p in refs]}")
            removed += 1
            return ""
        return match.group(0)

    text = BARE_MOD.sub(repl_bare, text)
    if text != original:
        atomic_write(src, text)
    return removed


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    total = 0
    for src in sorted(root.rglob("*.rs")):
        if any(part in {".git", "target"} for part in src.parts):
            continue
        total += clean_one(src)
    print(f"stale module cleanup removed {total} declarations")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
