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
    """The final terminator is not a line, so both modes accept the record the
    host scripts actually write."""
    assert parse_key_value_record(RECORD, REQUIRED, allow_body=True) is not None
    assert parse_key_value_record(RECORD.rstrip("\n"), REQUIRED) is not None


# `host-reboot.sh` and `host-diagnostics.sh` terminate every field with a
# literal `\n`. `str.splitlines` breaks on these as well, so using it here would
# read one field value as two lines.
@pytest.mark.parametrize(
    "separator",
    ["\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"],
)
def test_only_a_newline_separates_fields(separator: str):
    record = f"version=1\nrequest_id=abc\nstatus=a{separator}b\n"
    assert parse_key_value_record(record, REQUIRED) == {
        "version": "1",
        "request_id": "abc",
        "status": f"a{separator}b",
    }
    # With a body allowed the same character used to truncate the value in
    # place, quietly handing the caller half of what the host published.
    assert parse_key_value_record(record, REQUIRED, allow_body=True) == {
        "version": "1",
        "request_id": "abc",
        "status": f"a{separator}b",
    }


def test_a_carriage_return_stays_inside_the_value_it_belongs_to():
    """A CRLF record is not the contract, so `\\r` is data, not a separator."""
    assert parse_key_value_record("version=1\r\nrequest_id=abc\nstatus=ok\n", REQUIRED) == {
        "version": "1\r",
        "request_id": "abc",
        "status": "ok",
    }


def test_the_body_is_never_scanned_when_it_is_allowed():
    """Reading three header fields must not cost the size of the payload.

    `services/diagnostics.py` polls the host report at 20 Hz for the whole host
    budget, and a report carries up to 256 KiB per section. Every field-shaped
    line below sits in the body, so a parser that reached them would both waste
    that work and let the payload contribute fields.
    """
    body = "\n" + "".join(f"planted{index}=value\n" for index in range(20_000))

    parsed = parse_key_value_record(RECORD + body, REQUIRED, allow_body=True)

    assert parsed == {"version": "1", "request_id": "abc", "status": "complete"}
    # A body that repeats a header field is still body, not a duplicate key.
    assert parse_key_value_record(RECORD + "\nversion=2\n", REQUIRED, allow_body=True) == {
        "version": "1",
        "request_id": "abc",
        "status": "complete",
    }


def test_a_blank_line_is_a_rejection_unless_a_body_is_allowed():
    assert parse_key_value_record(RECORD + "\n", REQUIRED) is None
    assert parse_key_value_record(RECORD + "\n", REQUIRED, allow_body=True) is not None


@pytest.mark.parametrize("record", ["\n" + RECORD, "\n"])
def test_a_record_that_opens_with_a_blank_line_carries_no_header(record):
    """The blank line ends the header wherever it falls, including first.

    A report whose header is empty has published no fields, so it can satisfy no
    contract -- it must not be read as the body-bearing record it resembles.
    """
    assert parse_key_value_record(record, REQUIRED, allow_body=True) is None
    assert parse_key_value_record(record, REQUIRED) is None


def test_strict_json_decoder_reports_excessive_nesting_as_a_document_error():
    value = "[" * 20_000 + "0" + "]" * 20_000
    with pytest.raises(JsonDocumentError, match="nesting is too deep"):
        load_json_document(value)
