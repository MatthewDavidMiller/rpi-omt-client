#!/usr/bin/env python3
"""Generate a deterministic CycloneDX inventory from the final Alpine runtime."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cargo_lock import LockGraph  # noqa: E402

FIRST_PARTY_DISTRIBUTION = "rpi-omt-client"
# The image ships the receiver binary alone; the deployer never enters it.
RUNTIME_ROOTS = ["omt-receiver"]


def command(*arguments: str) -> str:
    return subprocess.run(
        arguments,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def license_value(value: str) -> dict[str, object]:
    normalized = value.strip() or "NOASSERTION"
    if re.fullmatch(r"[A-Za-z0-9.+()-]+", normalized):
        return {"expression": normalized}
    return {"license": {"name": normalized}}


def metadata_value(distribution: importlib.metadata.Distribution, key: str) -> str:
    """Read one optional distribution header without assuming a mapping API.

    ``Distribution.metadata`` is typed as the ``PackageMetadata`` protocol, which
    does not expose ``get``. Test membership first: indexing a missing header
    returns None today but is deprecated and will raise, and the implicit-None
    lookup emits a DeprecationWarning during the image build.
    """
    if key not in distribution.metadata:
        return ""
    value = distribution.metadata[key]
    return str(value) if value else ""


def alpine_components() -> list[dict[str, object]]:
    components: list[dict[str, object]] = []
    for name in sorted(command("apk", "info").splitlines(), key=str.casefold):
        versioned = command("apk", "info", "-e", "-v", name).strip()
        version = versioned.removeprefix(f"{name}-")
        details = command("apk", "info", "-a", name)
        match = re.search(r"\n[^:\n]+ license:\n([^\n]+)", details)
        license_name = match.group(1).strip() if match else "NOASSERTION"
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


def python_components() -> list[dict[str, object]]:
    components: list[dict[str, object]] = []
    for distribution in sorted(
        importlib.metadata.distributions(),
        key=lambda item: metadata_value(item, "Name").casefold(),
    ):
        name = metadata_value(distribution, "Name")
        # The application itself is installed as a wheel in the same venv. It is
        # already this document's metadata.component, and it has no PyPI
        # identity, so listing it here would misreport first-party code as a
        # third-party dependency.
        if not name or name.lower().replace("_", "-") == FIRST_PARTY_DISTRIBUTION:
            continue
        license_name = (
            metadata_value(distribution, "License-Expression")
            or metadata_value(distribution, "License")
            or "NOASSERTION"
        )
        version = distribution.version
        components.append(
            {
                "type": "library",
                "name": name,
                "version": version,
                "purl": f"pkg:pypi/{name.lower().replace('_', '-')}@{version}",
                "licenses": [license_value(license_name)],
                "properties": [{"name": "runtime", "value": "Python"}],
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
    arguments = parser.parse_args()
    components = alpine_components() + python_components() + rust_components(arguments.cargo_lock)
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
