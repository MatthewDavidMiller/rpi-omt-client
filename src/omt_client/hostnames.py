"""The one ASCII host grammar shared by every OMT target this app accepts.

`discovery.is_valid_direct_target` and `network_config.normalize_discovery_server`
take the same authority -- an IP literal or a DNS name of ASCII labels -- but
answer different questions about it, so each used to carry its own copy of the
rules. Divergence is not a cosmetic risk here: the receiver's `TargetValidator`
enforces this grammar too, so a host accepted on only one side is a target the
web UI saves and the receiver then refuses to play.
"""

from __future__ import annotations

import ipaddress

MAX_HOST_LENGTH = 253
MAX_LABEL_LENGTH = 63


def is_ascii(value: str) -> bool:
    """Report whether every character is ASCII.

    Kept separate from `canonical_host` so a caller can tell an operator that
    they pasted an internationalized name rather than a malformed one.
    """
    return all(ord(character) < 128 for character in value)


def _is_valid_label(label: str) -> bool:
    return (
        0 < len(label) <= MAX_LABEL_LENGTH
        and label[0].isalnum()
        and label[-1].isalnum()
        and all(character.isalnum() or character == "-" for character in label)
    )


def canonical_host(host: str) -> str | None:
    """Return `host` in canonical form, or None when it is not a valid host.

    An IPv6 literal comes back bracketed so a caller can splice it straight
    into `omt://host:port`; IPv4 comes back in its normalized form and a DNS
    name lowercased. `label[0].isalnum()` is Unicode-aware in Python, so the
    ASCII guard has to run first for the label rules to mean what they say.
    """
    if not host or not is_ascii(host):
        return None
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        pass
    else:
        return f"[{address}]" if address.version == 6 else str(address)
    if len(host) > MAX_HOST_LENGTH:
        return None
    if not all(_is_valid_label(label) for label in host.split(".")):
        return None
    return host.lower()
