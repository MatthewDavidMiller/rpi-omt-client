#!/usr/bin/env python3
"""Generate a CycloneDX inventory for the self-contained Windows deployer."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--lock-file", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--version", required=True)
    arguments = parser.parse_args()
    lock = json.loads(Path(arguments.lock_file).read_text(encoding="utf-8"))
    components = []
    for name, record in lock["dependencies"]["net10.0"].items():
        if record["type"] == "Project":
            continue
        version = record["resolved"]
        components.append(
            {
                "type": "library",
                "name": name,
                "version": version,
                "purl": f"pkg:nuget/{name}@{version}",
            }
        )
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "Raspberry Pi OMT Client Deployer",
                "version": arguments.version,
                "licenses": [{"license": {"name": "LicenseRef-Proprietary"}}],
            }
        },
        "components": sorted(components, key=lambda item: item["name"].casefold()),
    }
    Path(arguments.output).write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
