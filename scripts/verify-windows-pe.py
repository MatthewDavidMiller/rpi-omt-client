#!/usr/bin/env python3
"""Verify that a publish result is a non-empty x86-64 Windows PE executable."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


def verify(path: Path) -> None:
    with path.open("rb") as executable:
        header = executable.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise ValueError("missing DOS MZ signature")
        pe_offset = struct.unpack_from("<I", header, 0x3C)[0]
        executable.seek(pe_offset)
        pe_header = executable.read(6)
        if len(pe_header) != 6 or pe_header[:4] != b"PE\0\0":
            raise ValueError("missing PE signature")
        machine = struct.unpack_from("<H", pe_header, 4)[0]
        if machine != 0x8664:
            raise ValueError(f"unexpected PE machine 0x{machine:04x}; expected x86-64 (0x8664)")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("executable", type=Path)
    args = parser.parse_args()
    verify(args.executable)
    print(f"Verified Windows x86-64 PE: {args.executable}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
