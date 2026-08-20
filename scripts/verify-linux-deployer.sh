#!/bin/bash
# Verify a Linux deployer binary against the shipping contract.
# Usage: ./scripts/verify-linux-deployer.sh <path-to-binary>
#
# The Linux deployer's portability claim is that one binary runs on every
# distribution -- glibc or musl, current or several years old -- because it
# resolves nothing at load time. That claim is a property of the ELF headers,
# so it is read back out of them here rather than assumed from the build flags
# that were meant to produce it.
#
# A dynamically linked build is the failure this exists to catch: it runs
# perfectly on the toolbox that produced it and fails on the operator's
# machine, which is the last place anyone wants to discover it.

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 <path-to-binary>" >&2
    exit 2
fi

EXECUTABLE="$1"
READELF="${READELF:-readelf}"

if [[ ! -s "${EXECUTABLE}" ]]; then
    echo "FAIL: ${EXECUTABLE} is missing or empty" >&2
    exit 1
fi
command -v "${READELF}" >/dev/null 2>&1 || {
    echo "ERROR: ${READELF} is required. Run: make install" >&2
    exit 1
}

failures=0
pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

headers="$("${READELF}" -hlWd "${EXECUTABLE}" 2>/dev/null || true)"
if [[ -z "${headers}" ]]; then
    echo "FAIL: ${EXECUTABLE} is not readable as an ELF object" >&2
    exit 1
fi

if grep -q 'Class:[[:space:]]*ELF64' <<< "${headers}" &&
   grep -q 'Machine:[[:space:]]*Advanced Micro Devices X86-64' <<< "${headers}"; then
    pass "artifact is a 64-bit x86-64 ELF object"
else
    fail "artifact is not a 64-bit x86-64 ELF object"
fi

# A program interpreter is the dynamic loader. Its absence is what makes the
# binary independent of the host's libc entirely, rather than merely
# compatible with some minimum version of one.
if grep -q 'Requesting program interpreter' <<< "${headers}"; then
    interpreter="$(sed -n 's/.*Requesting program interpreter: \([^]]*\)\].*/\1/p' <<< "${headers}")"
    fail "artifact requests a dynamic loader (${interpreter}), so it is not static"
else
    pass "artifact requests no dynamic loader"
fi

# NEEDED entries are shared libraries the loader would have to find. On a
# static build there are none; anything here is a library the operator's
# machine would have to supply at the right version.
mapfile -t needed < <(awk '/\(NEEDED\)/ { gsub(/[][]/, "", $NF); print $NF }' <<< "${headers}")
if ((${#needed[@]} == 0)); then
    pass "artifact declares no shared library dependencies"
else
    fail "artifact needs shared libraries: ${needed[*]}"
fi

# A static-PIE still relocates itself at load, so the kernel can randomise its
# base the way it does for a dynamic executable. A plain ET_EXEC static binary
# loads at a fixed address and loses that.
if grep -qE 'Type:[[:space:]]*DYN' <<< "${headers}"; then
    pass "artifact is position independent, so its base address is randomised"
else
    fail "artifact is not position independent"
fi

# An executable stack would opt the whole process out of no-execute
# enforcement.
stack_segment="$(grep -E 'GNU_STACK' <<< "${headers}" || true)"
if [[ -z "${stack_segment}" ]]; then
    pass "artifact declares no executable stack segment"
elif [[ "${stack_segment}" == *RWE* ]]; then
    fail "artifact requests an executable stack"
else
    pass "artifact enables data execution prevention"
fi

# A baked-in library search path is meaningless on a static binary and
# dangerous on any other, so it should simply not be there.
if grep -qE '\((RPATH|RUNPATH)\)' <<< "${headers}"; then
    fail "artifact hardcodes a library search path"
else
    pass "artifact hardcodes no library search path"
fi

if ((failures > 0)); then
    echo "Linux deployer verification failed with ${failures} finding(s)." >&2
    exit 1
fi
echo "Linux deployer artifact verified: ${EXECUTABLE}"
