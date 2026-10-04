#!/bin/bash
# Keep C/C++ and assembly confined to the first-party C tree.
#
# The project is C; what stays an exception is C anywhere else in the tree.
# What is still refused is C/C++ outside the directories the C gates build,
# lint, sanitize, and fuzz -- a vendored library or a stray helper there would
# ship without any of those checks -- and C++ anywhere. Assembly is allowed
# only under src/, where mk/c.mk builds it and the AArch64 suites check it
# against the portable C it replaces.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mapfile -t stray < <(
    git -C "${PROJECT_ROOT}" ls-files |
        grep -E '\.(c|h|cc|cpp|cxx|hh|hpp|hxx|S|s|asm)$' |
        grep -Ev '^(src|tests/c|tests/fuzz)/.*\.[ch]$|^src/.*\.S$' |
        while IFS= read -r path; do [[ -f "${PROJECT_ROOT}/${path}" ]] && printf '%s\n' "${path}"; done || true
)
if ((${#stray[@]})); then
    printf 'ERROR: C/C++ or assembly source outside the gated C tree: %s\n' "${stray[@]}" >&2
    exit 1
fi
echo "C sources are confined to src/, tests/c/, and tests/fuzz/; assembly to src/."
