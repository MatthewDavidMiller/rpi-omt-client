"""Validation and parsing for Open Media Transport source targets."""

from __future__ import annotations

import ipaddress
import json
import unicodedata
from urllib.parse import urlsplit

MAX_SOURCE_NAME_BYTES = 63
MAX_DISCOVERY_OUTPUT_BYTES = 256 * 1024
FORBIDDEN_UNICODE_CATEGORIES = {"Cc", "Cf", "Cs", "Zl", "Zp"}


def _text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", "replace")
    return str(value)


class OmtSourceChoice(str):
    """String-compatible OMT source carrying safe UI selection metadata."""

    def __new__(
        cls,
        name: str,
        *,
        backend: str = "OMT discovery",
        target: str | None = None,
    ) -> OmtSourceChoice:
        value = str.__new__(cls, name)
        value.name = name
        value.backend = backend
        value.address = ""
        value.target = target if target is not None else name
        value.selection_value = f"discovered|{value.target}"
        value.display_label = f"{name} — {backend}"
        return value


def is_valid_source_name(source: str) -> bool:
    if not isinstance(source, str) or not source or source != source.strip():
        return False
    if not unicodedata.is_normalized("NFC", source):
        return False
    if len(source.encode("utf-8")) > MAX_SOURCE_NAME_BYTES:
        return False
    return all(
        unicodedata.category(character) not in FORBIDDEN_UNICODE_CATEGORIES
        for character in source
    )


def is_valid_direct_target(target: str) -> bool:
    if not isinstance(target, str) or not target or len(target) > 512:
        return False
    if any(ord(character) < 32 or ord(character) == 127 for character in target):
        return False
    try:
        parsed = urlsplit(target)
        port = parsed.port
    except ValueError:
        return False
    if (
        parsed.scheme != "omt"
        or not parsed.hostname
        or port is None
        or not 1 <= port <= 65535
        or parsed.username is not None
        or parsed.password is not None
        or parsed.path
        or parsed.query
        or parsed.fragment
    ):
        return False
    host = parsed.hostname
    if any(ord(character) > 127 for character in host):
        return False
    try:
        ipaddress.ip_address(host)
        return True
    except ValueError:
        pass
    if len(host) > 253:
        return False
    labels = host.split(".")
    return all(
        label
        and len(label) <= 63
        and label[0].isalnum()
        and label[-1].isalnum()
        and all(character.isalnum() or character == "-" for character in label)
        for label in labels
    )


# Compatibility name used by the established service boundary.
is_valid_direct_address = is_valid_direct_target


def parse_source_selection(selection: str) -> tuple[str, str | None, str] | None:
    if not isinstance(selection, str):
        return None
    if selection.startswith("discovered|"):
        name = selection.removeprefix("discovered|")
        if is_valid_source_name(name):
            return name, None, "OMT discovery"
        return None
    if selection.startswith("direct|"):
        target = selection.removeprefix("direct|")
        if is_valid_direct_target(target):
            return target, target, "OMT direct"
        return None
    if is_valid_source_name(selection):
        return selection, None, "OMT discovery"
    return None


def parse_omt_sources(output: object) -> list[str]:
    """Parse the receiver's bounded JSON discovery array."""
    raw = _text(output)
    if len(raw.encode("utf-8")) > MAX_DISCOVERY_OUTPUT_BYTES:
        return []
    try:
        document = json.loads(raw)
    except (TypeError, ValueError):
        return []
    if not isinstance(document, list):
        return []
    sources: list[str] = []
    seen: set[str] = set()
    for entry in document:
        if not isinstance(entry, dict):
            continue
        name = entry.get("name")
        target = entry.get("target")
        if (
            isinstance(name, str)
            and isinstance(target, str)
            and name == target
            and is_valid_source_name(name)
            and name not in seen
        ):
            seen.add(name)
            sources.append(name)
    return sorted(sources)


def choices_from_receiver(output: object) -> list[OmtSourceChoice]:
    return [OmtSourceChoice(name) for name in parse_omt_sources(output)]
