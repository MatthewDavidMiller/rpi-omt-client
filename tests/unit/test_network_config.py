from typing import cast

import pytest

from omt_client.network_config import (
    OmtNetworkConfigurationError,
    empty_settings_xml,
    network_configuration_from_xml,
    normalize_discovery_server,
    update_network_configuration_xml,
)

# The read and update paths validate independently; both must reject these, so a
# case added here can never be covered on only one side.
MALFORMED_DOCUMENTS = (
    b"<Wrong />",
    b"<Settings><DiscoveryServer>a</DiscoveryServer>"
    b"<DiscoveryServer>b</DiscoveryServer></Settings>",
    b"<Settings>",
    # xml.etree expands internal entities, so this bounded document would
    # otherwise expand without limit.
    b"<!DOCTYPE Settings ["
    b'<!ENTITY a "aaaaaaaaaa">'
    b'<!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">'
    b'<!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">'
    b"]><Settings><DiscoveryServer>&c;</DiscoveryServer></Settings>",
    b'<!DOCTYPE Settings SYSTEM "http://192.0.2.1/x.dtd"><Settings />',
)


def test_network_configuration_round_trip_preserves_unmanaged_nodes():
    document = b"<Settings><Keep value='yes' /></Settings>"
    updated = update_network_configuration_xml(document, "Discovery.EXAMPLE")
    parsed = network_configuration_from_xml(updated)
    assert parsed["discovery_server"] == "omt://discovery.example:6399"
    assert b'<Keep value="yes"' in updated


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("", ""),
        ("192.0.2.1", "omt://192.0.2.1:6399"),
        ("omt://[2001:db8::1]:6400", "omt://[2001:db8::1]:6400"),
    ],
)
def test_discovery_server_normalization(value, expected):
    assert normalize_discovery_server(value) == expected


@pytest.mark.parametrize(
    "value",
    [
        "bad host",
        "omt://user@host:6399",
        "omt://host:0",
        "omt://host:1/path",
        "omt://host:6399?x=1",
        "omt://host:6399#x",
        # urlsplit reports these delimiters as absent because the component
        # after them is empty, so they need their own rejection.
        "omt://host:6399?",
        "omt://host:6399#",
        "omt://host:6399/",
        "host:6399?",
        "host\x00name",
        "host\x7fname",
        "x" * 513,
        "omt://host:99999999999999999999",
        "cámara.local",
        "-leading.example",
        "double..dot",
        "a" * 254,
    ],
)
def test_invalid_server_values(value):
    with pytest.raises(OmtNetworkConfigurationError):
        normalize_discovery_server(value)


def test_non_text_discovery_server_is_rejected():
    """The guard is reachable from untyped XML content, not just the typed form."""
    with pytest.raises(OmtNetworkConfigurationError, match="must be text"):
        normalize_discovery_server(cast(str, None))


@pytest.mark.parametrize("document", MALFORMED_DOCUMENTS)
def test_update_rejects_the_same_documents_as_read(document):
    """update_network_configuration_xml re-validates independently of the read
    path, so an unsafe stored document cannot be silently rewritten."""
    with pytest.raises(OmtNetworkConfigurationError):
        update_network_configuration_xml(document, "192.0.2.1")


def test_update_rejects_an_invalid_server_for_a_valid_document():
    with pytest.raises(OmtNetworkConfigurationError):
        update_network_configuration_xml(empty_settings_xml(), "bad host")


def test_clearing_the_discovery_server_round_trips_to_empty():
    configured = update_network_configuration_xml(empty_settings_xml(), "192.0.2.1")
    cleared = update_network_configuration_xml(configured, "")
    assert network_configuration_from_xml(cleared)["discovery_server"] == ""
    assert network_configuration_from_xml(cleared)["error"] == ""


def test_an_empty_settings_document_reads_as_unconfigured():
    assert network_configuration_from_xml(empty_settings_xml())["discovery_server"] == ""


@pytest.mark.parametrize("document", MALFORMED_DOCUMENTS)
def test_read_rejects_wrong_root_duplicates_and_malformed_values(document):
    with pytest.raises(OmtNetworkConfigurationError):
        network_configuration_from_xml(document)


def test_doctype_rejection_names_the_declaration_and_spares_comments():
    with pytest.raises(OmtNetworkConfigurationError, match="doctype or entities"):
        network_configuration_from_xml(b"<!doctype Settings><Settings />")
    document = b"<Settings><!-- an ordinary comment --><Keep /></Settings>"
    assert network_configuration_from_xml(document)["discovery_server"] == ""
    assert b"Keep" in update_network_configuration_xml(document, "192.0.2.1")
