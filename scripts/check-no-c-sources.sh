#!/bin/bash
# Keep the tree Rust: no C or C++ source anywhere, and assembly only where the
# decoder's AArch64 kernels live.
#
# Code in another language would ship without the gates the Rust tree has --
# the lints, the unsafe allowlist, the sanitizer suites, and the fuzzers. The
# two kernels under crates/omt-vmx/asm are the one exception: global_asm!
# assembles them into omt-vmx, and the AArch64 suites (scripts/test-arm64.sh)
# check them bit for bit against the portable Rust they replace. The ABI
# probe's C (tests/abi) is generated at gate time and never committed.
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mapfile -t stray < <(
    git -C "${PROJECT_ROOT}" ls-files --cached --others --exclude-standard |
        grep -E '\.(c|h|cc|cpp|cxx|hh|hpp|hxx|S|s|asm|inc)$' |
        grep -Ev '^crates/omt-vmx/asm/[a-z0-9_]+\.s$' |
        while IFS= read -r path; do [[ -f "${PROJECT_ROOT}/${path}" ]] && printf '%s\n' "${path}"; done || true
)
if ((${#stray[@]})); then
    printf 'ERROR: C/C++ or assembly source outside crates/omt-vmx/asm: %s\n' "${stray[@]}" >&2
    exit 1
fi
echo "No C or C++ sources; assembly is confined to crates/omt-vmx/asm."
