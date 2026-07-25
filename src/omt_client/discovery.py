"""Validation and parsing for Open Media Transport source targets."""

from __future__ import annotations

import json
import unicodedata
from dataclasses import dataclass
from urllib.parse import urlsplit

from .hostnames import canonical_host

MAX_SOURCE_NAME_BYTES = 63
MAX_DISCOVERY_OUTPUT_BYTES = 256 * 1024
FORBIDDEN_UNICODE_CATEGORIES = {"Cc", "Cf", "Cs", "Zl", "Zp"}


def _text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", "replace")
    return str(value)


@dataclass(frozen=True)
class OmtSourceChoice:
    """Typed OMT source metadata exposed to the presentation layer."""

    name: str
    backend: str = "OMT discovery"

    @property
    def address(self) -> str:
        return ""

    @property
    def selection_value(self) -> str:
        return f"discovered|{self.name}"

    @property
    def display_label(self) -> str:
        return f"{self.name} — {self.backend}"


def is_valid_source_name(source: str) -> bool:
    if not isinstance(source, str) or not source or source != source.strip():
        return False
    if not unicodedata.is_normalized("NFC", source):
        return False
    try:
        encoded = source.encode("utf-8")
    except UnicodeEncodeError:
        return False
    if len(encoded) > MAX_SOURCE_NAME_BYTES:
        return False
    return all(
        unicodedata.category(character) not in FORBIDDEN_UNICODE_CATEGORIES for character in source
    )


def is_valid_direct_target(target: str) -> bool:
    if (
        not isinstance(target, str)
        or not target
        or len(target) > 512
        or not target.startswith("omt://")
    ):
        return False
    if any(ord(character) < 32 or ord(character) == 127 for character in target):
        return False
    # urlsplit reports an empty query for "omt://host:1?" and an empty fragment
    # for "omt://host:1#", both of which the checks below would read as absent.
    # The receiver's TargetValidator rejects these delimiters outright, so a
    # target accepted here but refused there would persist and never play.
    if any(delimiter in target[len("omt://") :] for delimiter in "/?#"):
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
    return canonical_host(parsed.hostname) is not None


def parse_source_selection(selection: str) -> tuple[str, str | None, str] | None:
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
