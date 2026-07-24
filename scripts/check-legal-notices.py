#!/usr/bin/env python3
"""Fail release checks when shipped dependencies or legal surfaces drift."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COPYRIGHT = "Copyright © 2026 Matthew David Miller. All rights reserved."


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


def windows_packages() -> set[str]:
    document = json.loads(
        (ROOT / "src/deployer/RpiOmt.Deployer.App/packages.lock.json").read_text(encoding="utf-8")
    )
    return {
        name.lower()
        for name, record in document["dependencies"]["net10.0"].items()
        if record["type"] != "Project"
    }


def main() -> int:
    notices = (ROOT / "THIRD_PARTY_NOTICES.txt").read_text(encoding="utf-8").lower()
    for package in sorted(python_packages()):
        if package not in notices:
            fail(f"Python runtime package is missing from notices: {package}")
    for package in sorted(windows_packages()):
        family = package.split(".", 1)[0]
        if package not in notices and family not in notices:
            fail(f"Windows runtime package is missing from notices: {package}")

    for path in (
        ROOT / "LICENSE",
        ROOT / "src/omt_client/templates/about.html",
        ROOT / "src/deployer/RpiOmt.Deployer.App/BuildInformation.cs",
    ):
        require_text(path, COPYRIGHT)

    for component in ("libomtnet", "libvmx", "omtplayer"):
        license_path = ROOT / f"third_party/omt/{component}/LICENSE.txt"
        require_text(license_path, "MIT License")
        require_text(ROOT / "third_party/omt/PROVENANCE.md", component)

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
    for forbidden in ("gst-plugin-ndi", "libndi"):
        if forbidden in dockerfile:
            fail(f"Dockerfile still references legacy software: {forbidden}")

    deployer_project = (
        (ROOT / "src/deployer/RpiOmt.Deployer.App/RpiOmt.Deployer.App.csproj")
        .read_text(encoding="utf-8")
        .lower()
    )
    deployer_build_info = (
        (ROOT / "src/deployer/RpiOmt.Deployer.App/BuildInformation.cs")
        .read_text(encoding="utf-8")
        .lower()
    )
    for required in (
        "microsoft.netcore.app.runtime.win-x64",
        "skiasharp.nativeassets.win32",
    ):
        if required not in deployer_project:
            fail(f"Windows About resources omit required notice input: {required}")
    for resource in (
        "rpiomt.dotnetthirdpartynotices.txt",
        "rpiomt.skiaharfbuzzthirdpartynotices.txt",
    ):
        if resource not in deployer_build_info:
            fail(f"Windows About does not render required notice resource: {resource}")

    manifest = (ROOT / "deploy/manifest-v2.txt").read_text(encoding="ascii").splitlines()[1:]
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
        f"{len(windows_packages())} Windows runtime packages covered."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
