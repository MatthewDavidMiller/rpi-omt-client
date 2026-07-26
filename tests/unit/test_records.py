"""Shared host-record parsing.

`services/diagnostics.py` and `services/host_system.py` both read `key=value`
records written by root-owned host scripts, so the two callers must not be able
to drift apart on what they accept.
"""

from __future__ import annotations

import pytest

from omt_client.json_document import JsonDocumentError, load_json_document
from omt_client.records import parse_key_value_record

REQUIRED = frozenset({"version", "request_id", "status"})
RECORD = "version=1\nrequest_id=abc\nstatus=complete\n"


def test_exact_field_set_is_returned():
    assert parse_key_value_record(RECORD, REQUIRED) == {
        "version": "1",
        "request_id": "abc",
        "status": "complete",
    }


def test_empty_values_and_embedded_separators_are_preserved():
    parsed = parse_key_value_record("version=1\nrequest_id=\nstatus=a=b\n", REQUIRED)
    assert parsed == {"version": "1", "request_id": "", "status": "a=b"}


@pytest.mark.parametrize(
    "record",
    [
        "version=1\nrequest_id=abc\n",  # missing a required field
        RECORD + "extra=1\n",  # unexpected field
        "version=1\nversion=2\nrequest_id=abc\nstatus=complete\n",  # duplicate
        "version=1\nrequest_id abc\nstatus=complete\n",  # no separator
        "",
    ],
)
def test_records_that_do_not_match_the_contract_are_rejected(record):
    assert parse_key_value_record(record, REQUIRED) is None


def test_body_is_ignored_only_when_the_caller_opts_in():
    with_body = RECORD + "\nfree-form payload\nnot=a=field\n"
    assert parse_key_value_record(with_body, REQUIRED, allow_body=True) == {
        "version": "1",
        "request_id": "abc",
        "status": "complete",
    }
    assert parse_key_value_record(with_body, REQUIRED) is None


def test_a_trailing_newline_is_not_treated_as_a_body():
    """splitlines() drops the final terminator, so both modes accept the record
    the host scripts actually write."""
    assert parse_key_value_record(RECORD, REQUIRED, allow_body=True) is not None
    assert parse_key_value_record(RECORD.rstrip("\n"), REQUIRED) is not None


def test_strict_json_decoder_reports_excessive_nesting_as_a_document_error():
    value = "[" * 20_000 + "0" + "]" * 20_000
    with pytest.raises(JsonDocumentError, match="nesting is too deep"):
        load_json_document(value)
