#!/usr/bin/env python3
"""Fail release checks when shipped dependencies or legal surfaces drift."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COPYRIGHT = "Copyright (c) 2026 Matthew David Miller"


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def require_text(path: Path, value: str) -> None:
    if value not in path.read_text(encoding="utf-8"):
        fail(f"{path.relative_to(ROOT)} does not contain required text: {value}")


def python_packages() -> set[str]:
    text = (ROOT / "requirements/runtime.txt").read_text(encoding="utf-8")
    return {
        match.group(1).lower().replace("_", "-")
        for match in re.finditer(r"^([A-Za-z0-9_.-]+)==", text, re.MULTILINE)
    }


def main() -> int:
    notices = (ROOT / "THIRD_PARTY_NOTICES.txt").read_text(encoding="utf-8").lower()
    for package in sorted(python_packages()):
        if package not in notices:
            fail(f"Python runtime package is missing from notices: {package}")
    for package in ("sdl 3.4.8", "nuklear 4.13.3", "libssh2 1.11.1"):
        if package not in notices:
            fail(f"Native deployer dependency is missing from notices: {package}")
    # The Windows cross build links these into the shipped .exe, so they are
    # redistributed even though no repository file carries their source.
    for package in ("mingw-w64 runtime", "gcc runtime library", "gcc-exception-3.1"):
        if package not in notices:
            fail(f"Windows deployer runtime is missing from notices: {package}")

    for path in (
        ROOT / "LICENSE",
        ROOT / "src/omt_client/templates/about.html",
        ROOT / "src/native/deployer/ui_main.c",
    ):
        require_text(path, COPYRIGHT)
    require_text(ROOT / "LICENSE", "MIT License")

    require_text(ROOT / "third_party/omt/libvmx/LICENSE.txt", "MIT License")
    for component in ("libomtnet", "libvmx", "omtplayer"):
        require_text(ROOT / "third_party/omt/PROVENANCE.md", component)
        if component not in notices:
            fail(f"OMT attribution is missing from notices: {component}")

    dockerfile = (ROOT / "deploy/Dockerfile").read_text(encoding="utf-8").lower()
    for required in (
        "third_party/omt",
        "third_party_notices.txt",
        "generate-runtime-sbom.py",
        "runtime-sbom.cdx.json",
        "dist-info/licenses",
    ):
        if required not in dockerfile:
            fail(f"Dockerfile does not retain required legal input: {required}")

    dependency_lock = (ROOT / "cmake/NativeDependencies.cmake").read_text(encoding="utf-8")
    for required in ("SDL3-3.4.8", "v4.13.3", "libssh2-1.11.1", "URL_HASH", "SHA256"):
        if required not in dependency_lock:
            fail(f"Native dependency lock omits required input: {required}")

    manifest = (ROOT / "deploy/manifest-v3.txt").read_text(encoding="ascii").splitlines()[1:]
    required_artifacts = {
        "LICENSE",
        "THIRD_PARTY_NOTICES.txt",
        "THIRD_PARTY_SOURCE.md",
    }
    missing = required_artifacts.difference(manifest)
    if missing:
        fail(f"deployment manifest omits legal artifacts: {sorted(missing)}")

    print(
        f"Legal notice check passed: {len(python_packages())} Python and "
        "3 native deployer runtime packages covered."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
