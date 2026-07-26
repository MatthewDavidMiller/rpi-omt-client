"""Strict JSON decoding for schema-bound application records."""

from __future__ import annotations

import json
from typing import NoReturn


class JsonDocumentError(ValueError):
    """Raised when a JSON document has an ambiguous or unsupported encoding."""


def _unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    document: dict[str, object] = {}
    for key, value in pairs:
        if key in document:
            raise JsonDocumentError(f"duplicate JSON key: {key}")
        document[key] = value
    return document


def _reject_constant(value: str) -> NoReturn:
    raise JsonDocumentError(f"non-standard JSON constant: {value}")


def load_json_document(value: str | bytes) -> object:
    """Decode UTF-8 JSON while rejecting duplicate keys and non-standard values.

    Python's default byte decoder also accepts UTF-16/UTF-32, silently keeps the
    last occurrence of a duplicate key, and accepts NaN/Infinity. Persistent
    records and cross-process status files have exact schemas, so those
    ambiguities are invalid at this boundary.
    """
    try:
        text = value.decode("utf-8") if isinstance(value, bytes) else value
    except UnicodeDecodeError as exc:
        raise JsonDocumentError(f"JSON is not valid UTF-8: {exc}") from exc
    try:
        document: object = json.loads(
            text,
            object_pairs_hook=_unique_object,
            parse_constant=_reject_constant,
        )
    except json.JSONDecodeError as exc:
        raise JsonDocumentError(str(exc)) from exc
    except RecursionError as exc:
        raise JsonDocumentError("JSON nesting is too deep") from exc
    return document
