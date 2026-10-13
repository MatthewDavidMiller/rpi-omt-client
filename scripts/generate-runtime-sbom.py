#!/usr/bin/env python3
"""Generate a deterministic CycloneDX inventory from the final Alpine runtime."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

# The image ships the receiver and Web binaries; the deployer never enters it.
# Both are first-party Rust with no third-party crates. What they link
# dynamically -- musl, alsa-lib, OpenSSL -- is an Alpine package, which the
# apk database records; what they carry statically is the Rust standard
# library of the pinned release in rust-toolchain.toml.
RUNTIME_ROOTS = ["omt-receiver", "omt-web"]


def license_value(value: str) -> dict[str, object]:
    normalized = value.strip() or "NOASSERTION"
    if re.fullmatch(r"[A-Za-z0-9.+()-]+", normalized):
        return {"expression": normalized}
    return {"license": {"name": normalized}}


def alpine_components(installed_database: str) -> list[dict[str, object]]:
    components: list[dict[str, object]] = []
    records = Path(installed_database).read_text(encoding="utf-8").split("\n\n")
    packages = []
    for record in records:
        fields = dict(
            line.split(":", 1)
            for line in record.splitlines()
            if ":" in line and line[:1] in {"P", "V", "L"}
        )
        if "P" in fields and "V" in fields:
            packages.append((fields["P"], fields["V"], fields.get("L", "NOASSERTION")))
    for name, version, license_name in sorted(packages, key=lambda item: item[0].casefold()):
        components.append(
            {
                "type": "library",
                "name": name,
                "version": version,
                "purl": f"pkg:apk/alpine/{name}@{version}",
                "licenses": [license_value(license_name)],
                "properties": [{"name": "distribution", "value": "Alpine Linux 3.23"}],
            }
        )
    return components


def rust_release(toolchain_file: str) -> str:
    match = re.search(
        r'^channel = "([^"]+)"$', Path(toolchain_file).read_text(encoding="utf-8"), re.M
    )
    if not match:
        raise SystemExit(f"{toolchain_file} names no channel")
    return match.group(1)


def first_party_components(version: str, rust: str) -> list[dict[str, object]]:
    components: list[dict[str, object]] = [
        {
            "type": "application",
            "name": name,
            "version": version,
            "licenses": [{"license": {"id": "MIT"}}],
            "properties": [{"name": "language", "value": "Rust"}],
        }
        for name in RUNTIME_ROOTS
    ]
    components.append(
        {
            "type": "library",
            "name": "rust-std",
            "version": rust,
            "purl": f"pkg:generic/rust-std@{rust}",
            "licenses": [{"expression": "MIT OR Apache-2.0"}],
        }
    )
    # The NFC tables compiled into both binaries are derived from this data.
    components.append(
        {
            "type": "data",
            "name": "unicode-character-database",
            "version": "16.0.0",
            "licenses": [{"license": {"id": "Unicode-3.0"}}],
        }
    )
    return components


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--version", default="unknown")
    parser.add_argument("--apk-installed", default="/lib/apk/db/installed")
    parser.add_argument("--rust-toolchain", required=True)
    arguments = parser.parse_args()
    components = alpine_components(arguments.apk_installed) + first_party_components(
        arguments.version, rust_release(arguments.rust_toolchain)
    )
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "Raspberry Pi OMT Client",
                "version": arguments.version,
                "licenses": [{"license": {"id": "MIT"}}],
            }
        },
        "components": sorted(
            components,
            key=lambda component: (
                str(component["name"]).casefold(),
                str(component["version"]),
            ),
        ),
    }
    Path(arguments.output).write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
