"""ASCII host grammar shared by discovery targets and Discovery Server."""

from __future__ import annotations

import pytest

from omt_client.hostnames import MAX_LABEL_LENGTH, canonical_host, is_ascii


@pytest.mark.parametrize(
    "value",
    [
        "example.com",
        "EXAMPLE.com",
        "host-1.example",
        "localhost",
        "192.0.2.1",
        "2001:db8::1",
        "::1",
        "a" * MAX_LABEL_LENGTH + ".example",
    ],
)
def test_canonical_host_accepts_valid_hosts(value: str):
    assert canonical_host(value) is not None


@pytest.mark.parametrize(
    "value",
    [
        "",
        "-leading.example",
        "trailing-.example",
        "double..dot",
        "under_score.example",
        "cámara.local",
        "a" * (MAX_LABEL_LENGTH + 1) + ".example",
        "a" * 254,
    ],
)
def test_canonical_host_rejects_invalid_hosts(value: str):
    assert canonical_host(value) is None


def test_canonical_host_brackets_ipv6_and_lowercases_dns():
    assert canonical_host("2001:db8::1") == "[2001:db8::1]"
    assert canonical_host("::1") == "[::1]"
    assert canonical_host("EXAMPLE.com") == "example.com"
    assert canonical_host("192.0.2.1") == "192.0.2.1"


def test_is_ascii_rejects_non_ascii_early():
    assert is_ascii("example.com")
    assert not is_ascii("cámara.local")
