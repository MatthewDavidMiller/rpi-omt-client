#!/bin/bash
# Keep C/C++ confined to the first-party C tree.
#
# The project is C; what stays an exception is C anywhere else in the tree.
# What is still refused is C/C++ outside the directories the C gates build,
# lint, sanitize, and fuzz -- a vendored library or a stray helper there would
# ship without any of those checks -- and C++ anywhere.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mapfile -t stray < <(
    git -C "${PROJECT_ROOT}" ls-files |
        grep -E '\.(c|h|cc|cpp|cxx|hh|hpp|hxx)$' |
        grep -Ev '^(src|tests/c|tests/fuzz)/.*\.[ch]$' |
        while IFS= read -r path; do [[ -f "${PROJECT_ROOT}/${path}" ]] && printf '%s\n' "${path}"; done || true
)
if ((${#stray[@]})); then
    printf 'ERROR: C/C++ source outside the gated C tree: %s\n' "${stray[@]}" >&2
    exit 1
fi
echo "C sources are confined to src/, tests/c/, and tests/fuzz/."
