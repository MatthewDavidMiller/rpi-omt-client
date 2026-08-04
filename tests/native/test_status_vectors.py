#!/usr/bin/env python3
"""Assert the native status producer against the shared Python contract."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


def main() -> int:
    probe = Path(sys.argv[1])
    vectors = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    failures: list[str] = []
    for projection in vectors["projections"]:
        result = subprocess.run(
            [probe, *projection["events"]], check=True, capture_output=True, text=True
        )
        document = json.loads(result.stdout)
        if set(document) != set(vectors["fields"]):
            failures.append(f"{projection['name']}: status fields differ")
        for field in ("state", "video_state", "audio_state"):
            if document.get(field) != projection[field]:
                failures.append(
                    f"{projection['name']}: {field} expected {projection[field]!r}, "
                    f"got {document.get(field)!r}"
                )
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("native playback-status vectors passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
