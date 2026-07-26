"""Detect untrusted Actions input contexts embedded in shell source."""

from __future__ import annotations

import re
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path

from workflow_yaml_run_scalars import parse_run_scalars

_EXPRESSION = re.compile(r"\$\{\{(?P<body>.*?)\}\}", re.DOTALL)
_INPUT_CONTEXT = re.compile(
    r"(?<![A-Za-z0-9_.])(?:"
    r"github\s*\.\s*event\s*\.\s*inputs"
    r"|inputs"
    r")(?![A-Za-z0-9_])"
)


@dataclass(frozen=True)
class DirectInputExpression:
    run_line: int
    context: str


def iter_run_scalars(text: str) -> Iterator[tuple[int, str]]:
    """Yield line number and value from structured YAML scalar nodes."""

    for scalar in parse_run_scalars(text):
        yield scalar.line, scalar.value


def _mask_expression_strings(body: str) -> str:
    characters = list(body)
    index = 0
    while index < len(characters):
        if characters[index] != "'":
            index += 1
            continue
        characters[index] = " "
        index += 1
        while index < len(characters):
            if characters[index] != "'":
                characters[index] = " "
                index += 1
                continue
            characters[index] = " "
            if index + 1 < len(characters) and characters[index + 1] == "'":
                characters[index + 1] = " "
                index += 2
                continue
            index += 1
            break
    return "".join(characters)


def find_direct_input_expressions(text: str) -> tuple[DirectInputExpression, ...]:
    findings: list[DirectInputExpression] = []
    for run_line, scalar in iter_run_scalars(text):
        for expression in _EXPRESSION.finditer(scalar):
            body = _mask_expression_strings(expression.group("body"))
            context_match = _INPUT_CONTEXT.search(body)
            if context_match is None:
                continue
            compact_context = re.sub(r"\s+", "", context_match.group(0))
            context = (
                "github.event.inputs"
                if compact_context.startswith("github.event.inputs")
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
