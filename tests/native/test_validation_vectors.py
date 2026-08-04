#!/usr/bin/env python3
"""Exercise the native validator against the Python/native shared vectors."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


def main() -> int:
    probe = Path(sys.argv[1])
    vectors = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    failures: list[str] = []
    for section, kind in (("source_names", "source"), ("direct_targets", "direct")):
        for vector in vectors[section]:
            if "\0" in vector["value"]:
                actual = False  # An embedded NUL cannot be represented by the native API.
            else:
                result = subprocess.run(
                    [probe, kind, vector["value"]], check=False, capture_output=True
                )
                actual = result.returncode == 0
            if actual is not vector["valid"]:
                failures.append(
                    f"{kind} {vector['value']!r}: expected {vector['valid']}, got {actual}"
                )
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("native validation vectors passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
