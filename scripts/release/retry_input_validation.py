"""Canonical validation for untrusted downstream retry inputs."""

from __future__ import annotations

import re
from collections.abc import Mapping
from typing import Any

from downstream_channels import DIGEST, RELEASE_REF, SHA40, SHA256, match, positive

CHANNEL_PRODUCERS = {
    "chocolatey": ".github/workflows/release-chocolatey-channel.yml",
    "snap_candidate": ".github/workflows/release-snap-channel.yml",
}

IDENTITY_INPUT_FIELDS = (
    "channel",
    "source_sha",
    "release_id",
    "release_ref",
    "idempotency_key",
)

PREPARE_POSITIVE_INPUT_FIELDS = (
    "receipt_run_id",
    "receipt_workflow_id",
    "receipt_artifact_id",
    "receipt_envelope_artifact_id",
    "prior_result_run_id",
    "prior_result_producer_workflow_id",
    "prior_result_artifact_id",
    "prior_result_envelope_artifact_id",
)

PREPARE_DIGEST_INPUT_FIELDS = (
    "receipt_artifact_digest",
    "receipt_envelope_artifact_digest",
    "prior_result_artifact_digest",
    "prior_result_envelope_artifact_digest",
)

PREPARE_SHA256_INPUT_FIELDS = (
    "receipt_predicate_sha256",
    "receipt_envelope_sha256",
    "prior_result_predicate_sha256",
    "prior_result_envelope_sha256",
)

PREPARE_EVIDENCE_INPUT_FIELDS = (
    *IDENTITY_INPUT_FIELDS,
    *PREPARE_POSITIVE_INPUT_FIELDS,
    *PREPARE_DIGEST_INPUT_FIELDS,
    *PREPARE_SHA256_INPUT_FIELDS,
    "prior_result_producer_workflow_path",
)

ACTION_ONLY_INPUT_FIELDS = (
    "receipt_run_workflow_id",
    "prior_result_run_workflow_id",
)

ACTION_INPUT_FIELDS = (*PREPARE_EVIDENCE_INPUT_FIELDS, *ACTION_ONLY_INPUT_FIELDS)

CANONICAL_POSITIVE = re.compile(r"[1-9][0-9]*")
IDEMPOTENCY_KEY = re.compile(r"rmux-downstream-v1:[0-9a-f]{64}")


def _required(values: Mapping[str, Any], field: str) -> Any:
    if field not in values:
        raise ValueError(f"retry input {field} is missing")
    return values[field]


def _label(field: str) -> str:
    return f"retry input {field.replace('_', ' ')}"


def canonical_positive(value: Any, label: str) -> int:
    rendered = match(value, CANONICAL_POSITIVE, label)
    return positive(int(rendered), label)


def validate_identity_inputs(values: Mapping[str, Any]) -> None:
    channel = _required(values, "channel")
    if not isinstance(channel, str) or channel not in CHANNEL_PRODUCERS:
        raise ValueError("retry input channel is not canonical")
    match(_required(values, "source_sha"), SHA40, _label("source_sha"))
    canonical_positive(_required(values, "release_id"), _label("release_id"))
    match(_required(values, "release_ref"), RELEASE_REF, _label("release_ref"))
    match(
        _required(values, "idempotency_key"),
        IDEMPOTENCY_KEY,
        _label("idempotency_key"),
    )


def validate_prepare_evidence_inputs(values: Mapping[str, Any]) -> None:
    validate_identity_inputs(values)
    for field in PREPARE_POSITIVE_INPUT_FIELDS:
        canonical_positive(_required(values, field), _label(field))
    for field in PREPARE_DIGEST_INPUT_FIELDS:
        match(_required(values, field), DIGEST, _label(field))
    for field in PREPARE_SHA256_INPUT_FIELDS:
        match(_required(values, field), SHA256, _label(field))

    channel = _required(values, "channel")
    producer_path = _required(values, "prior_result_producer_workflow_path")
    if producer_path != CHANNEL_PRODUCERS[channel]:
        raise ValueError("retry input producer workflow path differs from channel")


def validate_action_inputs(values: Mapping[str, Any]) -> None:
    validate_prepare_evidence_inputs(values)
    for field in ACTION_ONLY_INPUT_FIELDS:
        canonical_positive(_required(values, field), _label(field))

    if _required(values, "receipt_run_id") != _required(
        values, "prior_result_run_id"
    ):
        raise ValueError("retry input receipt and result run IDs differ")

    receipt_workflow_id = _required(values, "receipt_workflow_id")
    for field in (
        "receipt_run_workflow_id",
        "prior_result_run_workflow_id",
        "prior_result_producer_workflow_id",
    ):
        if _required(values, field) != receipt_workflow_id:
            raise ValueError("retry input workflow identities differ")
