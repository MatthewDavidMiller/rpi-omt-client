#!/usr/bin/env python3
"""Fail release checks when shipped dependencies or legal surfaces drift."""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COPYRIGHT = "Copyright (c) 2026 Matthew David Miller"


def fail(message: str) -> None:
    print(f"ERROR: {message}", file=sys.stderr)
    raise SystemExit(1)


def require_text(path: Path, value: str) -> None:
    if value not in path.read_text(encoding="utf-8"):
        fail(f"{path.relative_to(ROOT)} does not contain required text: {value}")


def main() -> int:
    notices = (ROOT / "THIRD_PARTY_NOTICES.txt").read_text(encoding="utf-8").lower()
    for package in (
        "serde/serde_json",
        "clap",
        "unicode-normalization",
        "zeroize",
        "egui",
        "eframe",
    ):
        if package not in notices:
            fail(f"Native deployer dependency is missing from notices: {package}")
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    registry_packages = [package for package in lock["package"] if "source" in package]
    if not registry_packages or any("checksum" not in package for package in registry_packages):
        fail("Cargo registry graph is not completely checksum locked")

    for path in (
        ROOT / "LICENSE",
        ROOT / "crates/omt-web/templates/about.html",
        ROOT / "crates/rpi-omt-deployer/src/main.rs",
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
        "cargo.lock",
        "third_party_notices.txt",
        "generate-runtime-sbom.py",
        "runtime-sbom.cdx.json",
        "omt-web",
    ):
        if required not in dockerfile:
            fail(f"Dockerfile does not retain required legal input: {required}")

    manifest = (ROOT / "deploy/manifest-v3.txt").read_text(encoding="ascii").splitlines()[1:]
    required_artifacts = {
        "LICENSE",
        "THIRD_PARTY_NOTICES.txt",
        "THIRD_PARTY_SOURCE.md",
    }
    missing = required_artifacts.difference(manifest)
    if missing:
        fail(f"deployment manifest omits legal artifacts: {sorted(missing)}")

    count = len(registry_packages)
    print(f"Legal notice check passed: {count} checksum-locked Rust packages covered.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
