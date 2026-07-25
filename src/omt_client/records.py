"""Shared parsing for the plain `key=value` records exchanged with the host.

`host-diagnostics.sh` and `host-reboot.sh` both answer the container through a
line-oriented record with a fixed field set. Both sit on the host trust
boundary, so they must agree on what counts as well formed: `=` is required,
duplicate keys are rejected, and the field set must match exactly.
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
    for line in value.splitlines():
        if allow_body and not line:
            break
        key, separator, field_value = line.partition("=")
        if not separator or key in fields:
            return None
        fields[key] = field_value
    return fields if set(fields) == set(required) else None
