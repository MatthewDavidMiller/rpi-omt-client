#!/usr/bin/env python3
"""Generate the CycloneDX inventory for the native deployment application."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

COMPONENTS = (
    ("SDL", "3.4.8", "pkg:github/libsdl-org/SDL@release-3.4.8"),
    ("Dear ImGui", "1.92.8", "pkg:github/ocornut/imgui@v1.92.8"),
    ("libssh2", "1.11.1", "pkg:generic/libssh2@1.11.1"),
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--version", required=True)
    arguments = parser.parse_args()
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "Raspberry Pi OMT Client Deployer",
                "version": arguments.version,
                "licenses": [{"license": {"id": "MIT"}}],
            }
        },
        "components": [
            {"type": "library", "name": name, "version": version, "purl": purl}
            for name, version, purl in COMPONENTS
        ],
    }
    Path(arguments.output).write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
