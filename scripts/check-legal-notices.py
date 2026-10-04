#!/usr/bin/env python3
"""Fail release checks when shipped dependencies or legal surfaces drift."""

from __future__ import annotations

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


def main() -> int:
    notices_text = (ROOT / "THIRD_PARTY_NOTICES.txt").read_text(encoding="utf-8")
    notices = notices_text.lower()
    # The third-party code the binaries contain: OpenSSL everywhere, the
    # Unicode data in the appliance, musl and the mingw runtime in the static
    # deployers. Each needs its licence text, not only its name.
    for required in (
        "openssl",
        "apache license",
        "unicode license v3",
        "musl libc",
        "mingw-w64 runtime",
        "gcc runtime library",
    ):
        if required not in notices:
            fail(f"THIRD_PARTY_NOTICES.txt omits a shipped component: {required}")
    for retired in ("cargo.lock", "egui", "ratatui", "rustls"):
        if retired in notices:
            fail(f"THIRD_PARTY_NOTICES.txt still lists a retired component: {retired}")

    # Every About surface an operator can reach: the appliance's Web page and
    # the deployer's terminal view, which reproduces LICENSE and the notices
    # embedded through the capsule.
    for path in (
        ROOT / "LICENSE",
        ROOT / "src/web/templates/about.html",
    ):
        require_text(path, COPYRIGHT)
    require_text(ROOT / "LICENSE", "MIT License")
    require_text(ROOT / "src/deploy/tui/ui.c", "dp_license_text(")
    require_text(ROOT / "src/deploy/tui/ui.c", "dp_notices_text(")
    require_text(ROOT / "tools/gen/gen_capsule.py", "THIRD_PARTY_NOTICES.txt")

    require_text(ROOT / "third_party/omt/libvmx/LICENSE.txt", "MIT License")
    for component in ("libomtnet", "libvmx", "omtplayer"):
        require_text(ROOT / "third_party/omt/PROVENANCE.md", component)
        if component not in notices:
            fail(f"OMT attribution is missing from notices: {component}")

    dockerfile = (ROOT / "deploy/Dockerfile").read_text(encoding="utf-8").lower()
    for required in (
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

    print("Legal notice check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
