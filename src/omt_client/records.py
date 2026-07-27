"""Shared parsing for the plain `key=value` records exchanged with the host.

`host-diagnostics.sh` and `host-reboot.sh` both answer the container through a
line-oriented record with a fixed field set. Both sit on the host trust
boundary, so they must agree on what counts as well formed: fields are
separated by `\n` and nothing else, `=` is required, duplicate keys are
rejected, and the field set must match exactly.
"""

from __future__ import annotations


def parse_key_value_record(
    value: str,
    required: set[str] | frozenset[str],
    *,
    allow_body: bool = False,
) -> dict[str, str] | None:
    """Return the record's fields, or None if it does not match `required`.

    With `allow_body`, the first blank line ends the header and whatever follows
    is ignored -- the host diagnostics report carries its payload that way.
    Without it, every line must be a field, so trailing content is a rejection.
    """
    fields: dict[str, str] = {}
    for line in _lines(value):
        if allow_body and not line:
            break
        key, separator, field_value = line.partition("=")
        if not separator or key in fields:
            return None
        fields[key] = field_value
    return fields if set(fields) == set(required) else None


def _lines(value: str) -> list[str]:
    """Split on the record separator the host scripts actually write.

    `str.splitlines` would also break on \\v, \\f, \\x1c-\\x1e, U+0085, U+2028,
    and U+2029. The producers terminate every field with a literal `\\n` and
    nothing else, so treating those as separators reads a single field value as
    two lines: with `allow_body` the value is silently truncated at the first
    one, and without it an otherwise valid record stops parsing. Both make a
    root-written answer unreadable for a character that is legal inside a value.
    """
    lines = value.split("\n")
    # A record ends with a terminator rather than a bare final field, so the
    # empty string after the last `\n` is not a line. A second one is: that is a
    # blank line, which ends the header or, without `allow_body`, is a rejection.
    if lines and not lines[-1]:
        lines.pop()
    return lines
