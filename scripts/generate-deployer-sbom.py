#!/usr/bin/env python3
"""Generate the CycloneDX inventory for the native deployment application.

The deployer is first-party Rust. Everything else it contains is listed here:
the Rust standard library, OpenSSL (linked statically on both platforms), the
GCC runtime library (the static unwinder on Linux, libgcc on Windows), and
the C runtime the static build carries -- musl on Linux, the mingw-w64
runtime on Windows.
Windows' own DLLs and the Universal CRT ship with the operating system and
are not part of the package.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

# The published package contains both deployer binaries and nothing else.
DEPLOYER_ROOTS = ["rpi-omt-deploy", "rpi-omt-deploy-tui"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--openssl-version", required=True)
    parser.add_argument("--rust-version", required=True)
    # The GCC runtime library both static links carry.
    parser.add_argument("--gcc-version", required=True)
    # Linux: the musl the static binaries embed.
    parser.add_argument("--musl-version")
    # Windows: the mingw-w64 runtime a static mingw link carries.
    parser.add_argument("--mingw-runtime-version")
    arguments = parser.parse_args()

    components: list[dict[str, object]] = [
        {
            "type": "application",
            "name": name,
            "version": arguments.version,
            "licenses": [{"license": {"id": "MIT"}}],
            "properties": [{"name": "language", "value": "Rust"}],
        }
        for name in DEPLOYER_ROOTS
    ]
    components.append(
        {
            "type": "library",
            "name": "rust-std",
            "version": arguments.rust_version,
            "purl": f"pkg:generic/rust-std@{arguments.rust_version}",
            "licenses": [{"expression": "MIT OR Apache-2.0"}],
        }
    )
    components.append(
        {
            "type": "library",
            "name": "GCC runtime library (libgcc)",
            "version": arguments.gcc_version,
            "purl": f"pkg:generic/gcc@{arguments.gcc_version}",
            "licenses": [{"expression": "GPL-3.0-or-later WITH GCC-exception-3.1"}],
        }
    )
    components.append(
        {
            "type": "library",
            "name": "openssl",
            "version": arguments.openssl_version,
            "purl": f"pkg:generic/openssl@{arguments.openssl_version}",
            "licenses": [{"license": {"id": "Apache-2.0"}}],
        }
    )
    if arguments.musl_version:
        components.append(
            {
                "type": "library",
                "name": "musl",
                "version": arguments.musl_version,
                "purl": f"pkg:generic/musl@{arguments.musl_version}",
                "licenses": [{"license": {"id": "MIT"}}],
            }
        )
    if arguments.mingw_runtime_version:
        components.append(
            {
                "type": "library",
                "name": "mingw-w64 runtime",
                "version": arguments.mingw_runtime_version,
                "purl": f"pkg:generic/mingw-w64@{arguments.mingw_runtime_version}",
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
                "licenses": [{"license": {"id": "MIT"}}],
            }
        },
        "components": components,
    }
    Path(arguments.output).write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
