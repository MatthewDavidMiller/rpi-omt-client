import pytest

from omt_client.network_config import (
    OmtNetworkConfigurationError,
    empty_settings_xml,
    network_configuration_from_xml,
    normalize_discovery_server,
    update_network_configuration_xml,
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
    ["bad host", "omt://user@host:6399", "omt://host:0", "omt://host:1/path"],
)
def test_invalid_server_values(value):
    with pytest.raises(OmtNetworkConfigurationError):
        normalize_discovery_server(value)


def test_xml_rejects_wrong_root_duplicates_and_malformed_values():
    assert network_configuration_from_xml(empty_settings_xml())["discovery_server"] == ""
    for value in (
        b"<Wrong />",
        b"<Settings><DiscoveryServer>a</DiscoveryServer><DiscoveryServer>b</DiscoveryServer></Settings>",
        b"<Settings>",
    ):
        with pytest.raises(OmtNetworkConfigurationError):
            network_configuration_from_xml(value)
