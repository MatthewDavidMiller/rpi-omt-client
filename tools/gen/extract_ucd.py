#!/usr/bin/env python3
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
"""Extract the Unicode 16.0.0 normalization data the C tables are built from.

Run on a host whose Python carries Unicode 16.0.0 (Python 3.14). The output,
third_party/unicode/16.0.0/nfc-data.txt, is committed, so the table generator
and its gate check never depend on the interpreter's own Unicode version --
the toolbox's Python is older.

Each line is `CODEPOINT;CCC;DECOMPOSITION;PRIMARY` for every code point with a
non-zero canonical combining class or a canonical decomposition: hex code
point, decimal class, space-separated hex mapping (empty when none), and 1 when
a two-part mapping is a primary composite (NFC recomposes it), else 0.
"""

import sys
import unicodedata
from pathlib import Path

UNICODE_VERSION = "16.0.0"
OUTPUT = Path(__file__).resolve().parents[2] / f"third_party/unicode/{UNICODE_VERSION}/nfc-data.txt"


def main() -> int:
    if unicodedata.unidata_version != UNICODE_VERSION:
        print(
            f"need Unicode {UNICODE_VERSION}, have {unicodedata.unidata_version}", file=sys.stderr
        )
        return 1
    lines = [f"# Unicode {UNICODE_VERSION} normalization extract; see tools/gen/extract_ucd.py"]
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF or 0xAC00 <= cp < 0xAC00 + 11172:
            continue
        ch = chr(cp)
        ccc = unicodedata.combining(ch)
        raw = unicodedata.decomposition(ch)
        canonical = raw and not raw.startswith("<")
        if not ccc and not canonical:
            continue
        mapping = raw if canonical else ""
        primary = int(
            bool(canonical) and len(raw.split()) == 2 and unicodedata.normalize("NFC", ch) == ch
        )
        lines.append(f"{cp:04X};{ccc};{mapping};{primary}")
    OUTPUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
