#!/usr/bin/env python3
"""Generate a deterministic CycloneDX inventory from the final Alpine runtime."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cargo_lock import LockGraph  # noqa: E402

# The image ships the receiver and Rust Web binaries; the deployer never enters it.
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


def rust_components(path: str) -> list[dict[str, object]]:
    return [
        {
            "type": "library",
            "name": name,
            "version": version,
            "purl": purl,
            "properties": [{"name": "runtime", "value": "Rust"}],
        }
        for name, version, purl in LockGraph(path).closure(RUNTIME_ROOTS)
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--version", default="unknown")
    parser.add_argument("--cargo-lock", required=True)
    parser.add_argument("--apk-installed", default="/lib/apk/db/installed")
    arguments = parser.parse_args()
    components = alpine_components(arguments.apk_installed) + rust_components(arguments.cargo_lock)
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
