#!/usr/bin/env python3
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
"""Generate tests/vectors/nfc/vectors.txt, the NFC oracle for tests/c/test_nfc.c.

Each line is `<input utf-8 hex> <nfc utf-8 hex>`, with Python's unicodedata as
the reference. The set covers every code point with a canonical mapping or a
non-zero combining class on its own, Hangul edge cases, and seeded random
sequences that mix starters, marks, and composites so reordering and blocking
are exercised.
"""

import random
import sys
import unicodedata
from pathlib import Path

UNICODE_VERSION = "16.0.0"
OUTPUT = Path(__file__).resolve().parents[2] / "tests/vectors/nfc/vectors.txt"


def render() -> str:
    if unicodedata.unidata_version != UNICODE_VERSION:
        raise SystemExit(f"need Unicode {UNICODE_VERSION}, have {unicodedata.unidata_version}")
    interesting = []
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        ch = chr(cp)
        if unicodedata.combining(ch) or (
            unicodedata.decomposition(ch) and not unicodedata.decomposition(ch).startswith("<")
        ):
            interesting.append(ch)
    cases = list(interesting)
    cases += ["가", "각", "각", "각ᆨ", "ᄀ̀ᅡ", "가", "힣", "힣"]
    rng = random.Random(15)
    starters = [c for c in interesting if not unicodedata.combining(c)] + list("aeiouAEIOU")
    marks = [c for c in interesting if unicodedata.combining(c)]
    for _ in range(6000):
        parts = []
        for _ in range(rng.randint(1, 6)):
            parts.append(rng.choice(starters if rng.random() < 0.4 else marks))
        cases.append("".join(parts))
    lines = [f"{c.encode().hex()} {unicodedata.normalize('NFC', c).encode().hex()}" for c in cases]
    return "\n".join(lines) + "\n"


def main() -> int:
    text = render()
    if sys.argv[1:] == ["--check"]:
        if OUTPUT.read_text(encoding="utf-8") != text:
            print(f"{OUTPUT} is stale; run {sys.argv[0]}", file=sys.stderr)
            return 1
        return 0
    OUTPUT.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
