#!/usr/bin/env python3
"""Normalize repeated test-only attributes introduced by legacy cleanup stages."""
from __future__ import annotations
import os
import re
import sys
from pathlib import Path


def atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + '.rmux-stage16-tmp')
    tmp.write_text(text, encoding='utf-8')
    os.replace(tmp, path)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    path = root / 'crates/rmux-server/src/handler_attach/registration.rs'
    if path.exists():
        old = path.read_text(encoding='utf-8')
        new = re.sub(
            r'(?:#\[cfg\(test\)\]\n)+(?=use crate::client_names::attached_client_name;)',
            '#[cfg(test)]\n',
            old,
        )
        if new != old:
            atomic_write(path, new)
            print(f'normalized {path}')
    print('stage16 idempotency normalization complete')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
