"""Validation and safe XML transformation for OMT discovery settings."""

from __future__ import annotations

import xml.etree.ElementTree as ET
from typing import cast
from urllib.parse import urlsplit

from .hostnames import canonical_host, is_ascii

DISCOVERY_SERVER_DEFAULT_PORT = 6399
DISCOVERY_SERVER_MAX_BYTES = 512


class OmtNetworkConfigurationError(RuntimeError):
    """Raised when OMT discovery settings cannot be safely represented."""


def normalize_discovery_server(value: str) -> str:
    if not isinstance(value, str):
        raise OmtNetworkConfigurationError("Discovery Server must be text.")
    value = value.strip()
    if not value:
        return ""
    if len(value.encode("utf-8")) > DISCOVERY_SERVER_MAX_BYTES or any(
        ord(character) < 32 or ord(character) == 127 for character in value
    ):
        raise OmtNetworkConfigurationError("Discovery Server contains unsupported characters.")
    candidate = value if value.startswith("omt://") else f"omt://{value}"
    # urlsplit reports an empty query/fragment for a bare trailing "?" or "#",
    # which the checks below would read as absent. Reject the delimiters
    # outright, matching is_valid_direct_target and the receiver's validator.
    if any(delimiter in candidate[len("omt://") :] for delimiter in "/?#"):
        raise OmtNetworkConfigurationError("Discovery Server must be a host or omt://host:port.")
    try:
        parsed = urlsplit(candidate)
        parsed_port = parsed.port
        port = DISCOVERY_SERVER_DEFAULT_PORT if parsed_port is None else parsed_port
    except ValueError as exc:
        raise OmtNetworkConfigurationError("Discovery Server is invalid.") from exc
    if (
        parsed.scheme != "omt"
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.path
        or parsed.query
        or parsed.fragment
        or not 1 <= port <= 65535
    ):
        raise OmtNetworkConfigurationError("Discovery Server must be a host or omt://host:port.")
    if not is_ascii(parsed.hostname):
        raise OmtNetworkConfigurationError("Discovery Server host must be ASCII.")
    canonical = canonical_host(parsed.hostname)
    if canonical is None:
        raise OmtNetworkConfigurationError("Discovery Server host is invalid.")
    return f"omt://{canonical}:{port}"


class _DoctypeDeclared(Exception):
    """Internal signal raised out of the parser target by `_NoDoctypeBuilder`."""


class _NoDoctypeBuilder(ET.TreeBuilder):
    """A tree builder that refuses any document type declaration.

    `xml.etree` expands internal entities, so a 64 KB document of nested
    declarations ("billion laughs") can exhaust memory, and a doctype can also
    name an external DTD. No legitimate `settings.xml` carries one, so the
    declaration is rejected outright rather than bounding the expansion.

    Expat calls this after it has decoded the document, so the guard sees the
    same characters the parser does. A textual pre-scan of the raw bytes cannot:
    an XML document declares its own encoding, so a UTF-16 `settings.xml`
    contains no literal "<!DOCTYPE" for such a scan to find while expat still
    reads and honours the declaration.
    """

    def doctype(self, name: str, pubid: str, system: str) -> None:
        raise _DoctypeDeclared


def _parse_settings(value: str | bytes) -> ET.Element:
    """Parse one bounded OMT settings document without entity expansion."""
    parser = ET.XMLParser(target=_NoDoctypeBuilder())
    try:
        return ET.fromstring(value, parser=parser)
    except _DoctypeDeclared as exc:
        raise OmtNetworkConfigurationError(
            "OMT settings XML must not declare a doctype or entities."
        ) from exc
    except (ET.ParseError, TypeError, ValueError) as exc:
        raise OmtNetworkConfigurationError(f"OMT settings XML is invalid: {exc}") from exc


def network_configuration_from_xml(value: str | bytes) -> dict[str, object]:
    root = _parse_settings(value)
    if root.tag != "Settings":
        raise OmtNetworkConfigurationError("OMT settings root must be <Settings>.")
    nodes = root.findall("DiscoveryServer")
    if len(nodes) > 1:
        raise OmtNetworkConfigurationError(
            "OMT settings contain duplicate DiscoveryServer entries."
        )
    raw = nodes[0].text or "" if nodes else ""
    server = normalize_discovery_server(raw)
    return {
        "discovery_server": server,
        "discovery_server_text": server,
        "error": "",
    }


def update_network_configuration_xml(value: str | bytes, server: str) -> bytes:
    normalized = normalize_discovery_server(server)
    root = _parse_settings(value)
    if root.tag != "Settings":
        raise OmtNetworkConfigurationError("OMT settings root must be <Settings>.")
    nodes = root.findall("DiscoveryServer")
    if len(nodes) > 1:
        raise OmtNetworkConfigurationError(
            "OMT settings contain duplicate DiscoveryServer entries."
        )
    node = nodes[0] if nodes else ET.SubElement(root, "DiscoveryServer")
    node.text = normalized
    ET.indent(root, space="  ")
    return cast(bytes, ET.tostring(root, encoding="utf-8", xml_declaration=True)) + b"\n"


def empty_settings_xml() -> bytes:
    return b'<?xml version="1.0" encoding="utf-8"?>\n<Settings />\n'
