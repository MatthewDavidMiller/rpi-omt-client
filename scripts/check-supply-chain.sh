#!/bin/bash
# Enforce the Rust supply-chain policy: cargo-deny and cargo-vet.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

missing=()
command -v cargo >/dev/null 2>&1 || missing+=("cargo")
command -v cargo-deny >/dev/null 2>&1 || missing+=("cargo-deny")
command -v cargo-vet >/dev/null 2>&1 || missing+=("cargo-vet")
if ((${#missing[@]} > 0)); then
    echo "ERROR: missing supply-chain tools: ${missing[*]}. Run: make install" >&2
    exit 1
fi

echo "Running cargo deny check..."
cargo deny --locked check

echo "Running cargo vet check..."
cargo vet check --locked --no-minimize-exemptions

echo "Supply-chain gates passed."
