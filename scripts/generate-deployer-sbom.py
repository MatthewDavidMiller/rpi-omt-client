#!/usr/bin/env python3
"""Generate the CycloneDX inventory for the native deployment application."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cargo_lock import LockGraph  # noqa: E402

# The published package contains both deployer binaries and nothing else.
DEPLOYER_ROOTS = ["rpi-omt-deploy", "rpi-omt-deployer"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--cargo-lock", required=True)
    # The Windows package statically links a compiler runtime the host package
    # gets from the operating system, so it inventories two extra components.
    parser.add_argument("--mingw-gcc-version")
    parser.add_argument("--mingw-runtime-version")
    arguments = parser.parse_args()
    components = LockGraph(arguments.cargo_lock).closure(DEPLOYER_ROOTS)
    if arguments.mingw_gcc_version:
        components.append(
            (
                "GCC runtime library (libgcc)",
                arguments.mingw_gcc_version,
                f"pkg:generic/gcc@{arguments.mingw_gcc_version}",
            )
        )
    if arguments.mingw_runtime_version:
        components.append(
            (
                "mingw-w64 runtime",
                arguments.mingw_runtime_version,
                f"pkg:generic/mingw-w64@{arguments.mingw_runtime_version}",
            )
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
                "licenses": [{"license": {"id": "MIT"}}],
            }
        },
        "components": [
            {"type": "library", "name": name, "version": version, "purl": purl}
            for name, version, purl in components
        ],
    }
    Path(arguments.output).write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
