"""Capture metadata compatibility for this SDK's generated response model."""

import pytest
from pydantic import BaseModel, ValidationError

from openai_codex.generated.v2_all import (
    CancelledCommandExecOutputCaptureEnd,
    CommandExecResponse,
    DrainDeadlineCommandExecOutputCaptureEnd,
    EofCommandExecOutputCaptureEnd,
    ReadErrorCommandExecOutputCaptureEnd,
)


@pytest.mark.parametrize(
    "capture_fields",
    [{}, {"stdoutCaptureEnd": None, "stderrCaptureEnd": None}],
    ids=["old_three_field_response", "explicit_unknown"],
)
def test_command_exec_unreported_capture_remains_unknown(
    capture_fields: dict[str, object],
) -> None:
    payload = {"exitCode": 0, "stdout": "original output", "stderr": "original error"}
    payload.update(capture_fields)

    response = CommandExecResponse.model_validate(payload)

    assert response.stdout_capture_end is None
    assert response.stderr_capture_end is None
    assert response.model_dump(by_alias=True, mode="json", exclude_unset=True) == payload
    assert response.model_dump(by_alias=True, mode="json") == {
        **payload,
        "stdoutCaptureEnd": None,
        "stderrCaptureEnd": None,
    }


@pytest.mark.parametrize("exit_code", [0, 23], ids=["root_success", "root_failure"])
@pytest.mark.parametrize(
    ("wire_field", "model_field"),
    [("stdoutCaptureEnd", "stdout_capture_end"), ("stderrCaptureEnd", "stderr_capture_end")],
    ids=["stdout", "stderr"],
)
@pytest.mark.parametrize(
    ("wire_end", "expected_type"),
    [
        ({"type": "eof"}, EofCommandExecOutputCaptureEnd),
        ({"type": "drainDeadline"}, DrainDeadlineCommandExecOutputCaptureEnd),
        ({"type": "cancelled"}, CancelledCommandExecOutputCaptureEnd),
        ({"type": "readError", "osError": None}, ReadErrorCommandExecOutputCaptureEnd),
        ({"type": "readError", "osError": 5}, ReadErrorCommandExecOutputCaptureEnd),
    ],
    ids=["eof", "drain_deadline", "cancelled", "read_error_unknown_code", "read_error_code"],
)
def test_command_exec_capture_variants_preserve_process_result_and_alias_wire(
    exit_code: int,
    wire_field: str,
    model_field: str,
    wire_end: dict[str, object],
    expected_type: type[BaseModel],
) -> None:
    payload = {
        "exitCode": exit_code,
        "stdout": "collected stdout prefix",
        "stderr": "collected stderr prefix",
        "stdoutCaptureEnd": None,
        "stderrCaptureEnd": None,
        wire_field: wire_end,
    }

    response = CommandExecResponse.model_validate(payload)

    capture_end = getattr(response, model_field)
    assert isinstance(capture_end.root, expected_type)
    assert response.model_dump(by_alias=True, mode="json") == payload
    assert (
        CommandExecResponse.model_validate_json(response.model_dump_json(by_alias=True)) == response
    )


@pytest.mark.parametrize("wire_field", ["stdoutCaptureEnd", "stderrCaptureEnd"])
def test_command_exec_invalid_capture_reason_is_not_treated_as_complete(wire_field: str) -> None:
    payload = {
        "exitCode": 0,
        "stdout": "collected stdout prefix",
        "stderr": "collected stderr prefix",
        wire_field: {"type": "unrecognizedReason"},
    }

    with pytest.raises(ValidationError):
        CommandExecResponse.model_validate(payload)
