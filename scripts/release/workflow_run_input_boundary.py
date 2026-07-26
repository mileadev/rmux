"""Detect untrusted Actions input contexts embedded in shell source."""

from __future__ import annotations

import re
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path

_RUN_KEY = re.compile(r"^(?P<indent> *)(?:-\s+)?run\s*:(?P<value>.*)$")
_FLOW_RUN_KEY = re.compile(r"(?:^|[{,])\s*run\s*:\s*(?P<value>.*)$")
_BLOCK_HEADER = re.compile(r"^[|>][1-9+-]*\s*(?:#.*)?$")
_EXPRESSION = re.compile(r"\$\{\{(?P<body>.*?)\}\}", re.DOTALL)
_INPUT_CONTEXT = re.compile(
    r"(?<![A-Za-z0-9_.])inputs(?:\.|\[)|github\.event\.inputs(?:\.|\[)"
)


@dataclass(frozen=True)
class DirectInputExpression:
    run_line: int
    context: str


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def iter_run_scalars(text: str) -> Iterator[tuple[int, str]]:
    """Yield line number and value for ordinary and flow-style `run` keys."""

    lines = text.splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        match = _RUN_KEY.match(line)
        if match is None:
            flow = _FLOW_RUN_KEY.search(line)
            if flow is not None:
                yield index + 1, flow.group("value")
            index += 1
            continue

        value = match.group("value").strip()
        if _BLOCK_HEADER.fullmatch(value) is None:
            yield index + 1, value
            index += 1
            continue

        key_indent = len(match.group("indent"))
        body: list[str] = []
        next_index = index + 1
        while next_index < len(lines):
            candidate = lines[next_index]
            if candidate.strip() and _indent(candidate) <= key_indent:
                break
            body.append(candidate)
            next_index += 1
        yield index + 1, "\n".join(body)
        index = next_index


def find_direct_input_expressions(text: str) -> tuple[DirectInputExpression, ...]:
    findings: list[DirectInputExpression] = []
    for run_line, scalar in iter_run_scalars(text):
        for expression in _EXPRESSION.finditer(scalar):
            compact = re.sub(r"\s+", "", expression.group("body"))
            context_match = _INPUT_CONTEXT.search(compact)
            if context_match is None:
                continue
            context = (
                "github.event.inputs"
                if context_match.group(0).startswith("github.event.inputs")
                else "inputs"
            )
            findings.append(DirectInputExpression(run_line, context))
    return tuple(findings)


def validate_no_direct_input_expressions(paths: Iterable[Path]) -> None:
    findings: list[str] = []
    for path in paths:
        for finding in find_direct_input_expressions(
            path.read_text(encoding="utf-8")
        ):
            findings.append(f"{path}:{finding.run_line} ({finding.context})")
    if findings:
        locations = ", ".join(findings)
        raise ValueError(
            "Actions input context is embedded directly in retry shell source: "
            f"{locations}"
        )
