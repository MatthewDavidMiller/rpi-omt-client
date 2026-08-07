#!/usr/bin/env python3
"""Resolve per-artifact dependency closures from a Cargo lockfile.

Both SBOM generators need the crates one shipped binary actually links, not
every crate in the workspace: the deployer must not claim to ship the
receiver's display and audio bindings, and the appliance image must not claim
to ship the deployer's SSH stack. Cargo.lock already records the resolved graph,
so the closure is computed from the lockfile alone. That keeps the runtime
generator runnable inside the container image, which has Python but no Cargo.
"""

from __future__ import annotations

import tomllib
from pathlib import Path
from typing import Any


class LockGraph:
    """The resolved package graph of one Cargo lockfile."""

    def __init__(self, path: str | Path) -> None:
        document = tomllib.loads(Path(path).read_text(encoding="utf-8"))
        packages = document.get("package", [])
        if not packages:
            raise SystemExit(f"{path} contains no resolved packages")
        # A lockfile may hold two versions of the same crate, so entries are
        # keyed by name and version and looked up by name when unambiguous.
        self._by_key: dict[tuple[str, str], dict[str, Any]] = {}
        self._by_name: dict[str, list[dict[str, Any]]] = {}
        for package in packages:
            key = (package["name"], package["version"])
            self._by_key[key] = package
            self._by_name.setdefault(package["name"], []).append(package)

    def _resolve(self, reference: str) -> dict[str, Any] | None:
        """Resolve a `name` or `name version` dependency reference."""
        parts = reference.split()
        if len(parts) >= 2:
            return self._by_key.get((parts[0], parts[1]))
        candidates = self._by_name.get(parts[0], [])
        return candidates[0] if len(candidates) == 1 else None

    def closure(self, roots: list[str]) -> list[tuple[str, str, str]]:
        """Every checksum-locked crate reachable from the named roots.

        Path and workspace members carry no checksum and are the artifacts
        themselves rather than third-party components, so they are traversed
        but not inventoried.
        """
        for root in roots:
            if root not in self._by_name:
                raise SystemExit(f"{root} is not a package in the lockfile")
        seen: set[tuple[str, str]] = set()
        pending = [self._by_name[root][0] for root in roots]
        while pending:
            package = pending.pop()
            key = (package["name"], package["version"])
            if key in seen:
                continue
            seen.add(key)
            for reference in package.get("dependencies", []):
                resolved = self._resolve(reference)
                if resolved is not None:
                    pending.append(resolved)
        return sorted(
            (
                name,
                version,
                f"pkg:cargo/{name}@{version}?checksum={self._by_key[(name, version)]['checksum']}",
            )
            for name, version in seen
            if "checksum" in self._by_key[(name, version)]
        )
