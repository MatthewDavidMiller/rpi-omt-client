#!/usr/bin/env python3
"""Enforce the deployer core's branch-coverage threshold from Cobertura XML."""

from __future__ import annotations

import argparse
import xml.etree.ElementTree as ET
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("coverage", type=Path)
    parser.add_argument("--minimum", type=float, default=95.0)
    args = parser.parse_args()

    root = ET.parse(args.coverage).getroot()  # noqa: S314 - local test artifact
    rate = float(root.attrib.get("branch-rate", "0")) * 100
    print(f"Core branch coverage: {rate:.2f}% (required: {args.minimum:.2f}%)")
    return 0 if rate + 1e-9 >= args.minimum else 1


if __name__ == "__main__":
    raise SystemExit(main())
