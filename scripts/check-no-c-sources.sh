#!/bin/bash
# Reject first-party or vendored checked-in C/C++ after the Rust cutover.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mapfile -t sources < <(
    git -C "${PROJECT_ROOT}" ls-files |
        grep -E '\.(c|h|cc|cpp|cxx|hh|hpp|hxx)$' |
        while IFS= read -r path; do [[ -f "${PROJECT_ROOT}/${path}" ]] && printf '%s\n' "${path}"; done || true
)
if ((${#sources[@]})); then
    printf 'ERROR: checked-in C/C++ source remains: %s\n' "${sources[@]}" >&2
    exit 1
fi
echo "No checked-in C/C++ sources found."
