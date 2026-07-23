import pytest
from discovery import (
    OmtSourceChoice,
    choices_from_receiver,
    is_valid_direct_target,
    is_valid_source_name,
    parse_omt_sources,
    parse_source_selection,
)


def test_discovery_json_is_validated_deduplicated_and_sorted():
    output = (
        '[{"name":"Studio","target":"Studio"},'
        '{"name":"Camera","target":"Camera"},'
        '{"name":"Studio","target":"Studio"},'
        '{"name":"Mismatch","target":"Other"}]'
    )
    assert parse_omt_sources(output) == ["Camera", "Studio"]
    assert [choice.name for choice in choices_from_receiver(output)] == [
        "Camera",
        "Studio",
    ]
    choice = OmtSourceChoice("Camera")
    assert choice.selection_value == "discovered|Camera"
    assert "OMT discovery" in choice.display_label


@pytest.mark.parametrize(
    "value",
    [
        "omt://host:1",
        "omt://192.0.2.1:65535",
        "omt://[2001:db8::1]:6400",
    ],
)
def test_direct_target_boundaries(value):
    assert is_valid_direct_target(value)
    assert parse_source_selection(f"direct|{value}") == (
        value,
        value,
        "OMT direct",
    )


@pytest.mark.parametrize(
    "value",
    ["host:1", "omt://user@host:1", "omt://host:0", "omt://host:1/path"],
)
def test_invalid_direct_targets(value):
    assert not is_valid_direct_target(value)


def test_source_names_require_nfc_bounded_printable_text():
    assert is_valid_source_name("Studio Camera")
    assert not is_valid_source_name(" Studio")
    assert not is_valid_source_name("e\u0301")
    assert not is_valid_source_name("x\n")
    assert parse_source_selection("discovered|Studio Camera") == (
        "Studio Camera",
        None,
        "OMT discovery",
    )
    assert parse_source_selection("discovered|bad\n") is None
    assert parse_omt_sources("not json") == []
