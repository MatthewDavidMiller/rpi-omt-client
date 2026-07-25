import json

import pytest
from conftest import REPO_ROOT

from omt_client.discovery import (
    OmtSourceChoice,
    is_valid_direct_target,
    is_valid_source_name,
    parse_omt_sources,
    parse_source_selection,
)
from omt_client.network_config import (
    OmtNetworkConfigurationError,
    normalize_discovery_server,
)

VECTORS = REPO_ROOT / "tests" / "schema" / "omt-target-vectors.json"

# Both validators take the same `omt://host:port`, so they must accept the same
# hosts. They read that host for different reasons -- one decides whether a
# target can be saved, the other canonicalizes a Discovery Server -- which is
# how they came to hold separate copies of the rules in the first place.
SHARED_HOST_GRAMMAR = (
    ("example.com", True),
    ("EXAMPLE.com", True),
    ("host-1.example", True),
    ("localhost", True),
    ("192.0.2.1", True),
    ("[2001:db8::1]", True),
    ("[::1]", True),
    ("a" * 63 + ".example", True),
    ("-leading.example", False),
    ("trailing-.example", False),
    ("double..dot", False),
    ("under_score.example", False),
    ("host_", False),
    ("cámara.local", False),
    ("[not:an:ip]", False),
    ("a" * 64 + ".example", False),
    ("a." * 127 + "a", False),
)


def test_discovery_json_is_validated_deduplicated_and_sorted():
    output = (
        '[{"name":"Studio","target":"Studio"},'
        '{"name":"Camera","target":"Camera"},'
        '{"name":"Studio","target":"Studio"},'
        '{"name":"Mismatch","target":"Other"}]'
    )
    assert parse_omt_sources(output) == ["Camera", "Studio"]
    choice = OmtSourceChoice("Camera")
    assert choice.selection_value == "discovered|Camera"
    assert "OMT discovery" in choice.display_label
    # Discovered sources carry no address; the templates render this field.
    assert choice.address == ""


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
    ("output", "expected"),
    [
        (None, []),
        (b'[{"name":"Camera","target":"Camera"}]', ["Camera"]),
        ('["Camera"]', []),
        ('[{"name":"Camera"}]', []),
        ('{"name":"Camera"}', []),
        ("[" + ",".join(['{"name":"a","target":"a"}'] * 20000) + "]", []),
    ],
    ids=["none", "bytes", "bare-strings", "missing-target", "not-an-array", "oversized"],
)
def test_discovery_output_must_be_a_bounded_typed_array(output, expected):
    assert parse_omt_sources(output) == expected


def test_source_selection_rejects_malformed_prefixed_values():
    assert parse_source_selection("direct|omt://host") is None
    assert parse_source_selection("direct|") is None
    assert parse_source_selection("discovered|") is None
    assert parse_source_selection("Bare Name") == ("Bare Name", None, "OMT discovery")
    assert parse_source_selection("bare\nname") is None


def test_source_names_require_nfc_bounded_printable_text():
    assert is_valid_source_name("Studio Camera")
    assert not is_valid_source_name(" Studio")
    assert not is_valid_source_name("e\u0301")
    assert not is_valid_source_name("x\n")
    assert not is_valid_source_name("\ud800")
    assert parse_source_selection("discovered|Studio Camera") == (
        "Studio Camera",
        None,
        "OMT discovery",
    )
    assert parse_source_selection("discovered|bad\n") is None
    assert parse_omt_sources("not json") == []


@pytest.mark.parametrize(("host", "valid"), SHARED_HOST_GRAMMAR)
def test_direct_targets_and_discovery_servers_accept_the_same_hosts(host, valid):
    """Pin the grammar both validators now take from `omt_client.hostnames`.

    Divergence would not be cosmetic: a host the web UI saves but the receiver's
    own validator refuses is a target that persists and never plays.
    """
    target = f"omt://{host}:6399"
    assert is_valid_direct_target(target) is valid
    try:
        normalize_discovery_server(target)
    except OmtNetworkConfigurationError:
        assert not valid
    else:
        assert valid


def test_python_and_receiver_share_target_validation_vectors():
    vectors = json.loads(VECTORS.read_text(encoding="utf-8"))
    for vector in vectors["source_names"]:
        assert is_valid_source_name(vector["value"]) is vector["valid"]
    for vector in vectors["direct_targets"]:
        assert is_valid_direct_target(vector["value"]) is vector["valid"]
