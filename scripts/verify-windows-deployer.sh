#!/bin/bash
# Verify a cross-compiled Windows deployer binary against the shipping contract.
# Usage: ./scripts/verify-windows-deployer.sh [--console] <path-to-exe>
#
# The Linux build host cannot run the artifact it produces, so the properties an
# operator depends on -- a 64-bit PE image, hardened, and self-contained on a
# stock Windows install -- are read out of the PE headers instead. The GUI
# deployer must declare the Windows GUI subsystem; pass --console for the CLI.

set -euo pipefail

EXPECT_GUI=1
if [[ "${1:-}" == "--console" ]]; then
    EXPECT_GUI=0
    shift
fi

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 [--console] <path-to-exe>" >&2
    exit 2
fi

EXECUTABLE="$1"
OBJDUMP="${OBJDUMP:-x86_64-w64-mingw32-objdump}"

# Only the properties that a Windows loader enforces.
DYNAMIC_BASE=$((0x0040))
NX_COMPAT=$((0x0100))
HIGH_ENTROPY_VA=$((0x0020))

if [[ ! -s "${EXECUTABLE}" ]]; then
    echo "FAIL: ${EXECUTABLE} is missing or empty" >&2
    exit 1
fi
command -v "${OBJDUMP}" >/dev/null 2>&1 || {
    echo "ERROR: ${OBJDUMP} is required. Run: make install" >&2
    exit 1
}

failures=0
pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

if "${OBJDUMP}" -f "${EXECUTABLE}" | grep -q 'file format pei-x86-64'; then
    pass "artifact is a 64-bit Windows PE image"
else
    fail "artifact is not a 64-bit Windows PE image"
fi

headers="$("${OBJDUMP}" -p "${EXECUTABLE}")"

if ((EXPECT_GUI)); then
    if grep -q 'Subsystem.*(Windows GUI)' <<< "${headers}"; then
        pass "artifact is a GUI subsystem binary, so it opens no console window"
    else
        fail "artifact does not declare the Windows GUI subsystem"
    fi
else
    if grep -q 'Subsystem.*(Windows CUI)' <<< "${headers}"; then
        pass "artifact is a console subsystem binary"
    else
        fail "artifact does not declare the Windows console subsystem"
    fi
fi

characteristics="$(awk '/^DllCharacteristics/ { print $2; exit }' <<< "${headers}")"
if [[ "${characteristics}" =~ ^[0-9a-fA-F]+$ ]]; then
    value=$((16#${characteristics}))
    for requirement in \
        "${DYNAMIC_BASE}:address space layout randomization" \
        "${NX_COMPAT}:data execution prevention" \
        "${HIGH_ENTROPY_VA}:64-bit address space randomization"; do
        bit="${requirement%%:*}"
        label="${requirement#*:}"
        if (( (value & bit) == bit )); then
            pass "artifact enables ${label}"
        else
            fail "artifact does not enable ${label}"
        fi
    done
else
    fail "artifact declares no DLL characteristics"
fi

# Static linking is what makes the .exe runnable on a stock Windows host. These
# names are the redistributables and third-party runtimes that a dynamic link
# would leave behind for the operator to chase down.
mapfile -t imports < <(awk '/DLL Name:/ { print tolower($3) }' <<< "${headers}")
if ((${#imports[@]} == 0)); then
    fail "artifact imports no DLLs at all, so its headers are unreadable"
fi
bundled=()
for import in "${imports[@]}"; do
    case "${import}" in
        lib*|sdl3*|zlib*|msvcp*|vcruntime*|api-ms-win-crt-*)
            bundled+=("${import}")
            ;;
    esac
done
if ((${#bundled[@]} == 0)); then
    pass "artifact imports only Windows system libraries (${#imports[@]} DLLs)"
else
    fail "artifact needs redistributable runtimes: ${bundled[*]}"
fi

if ((failures > 0)); then
    echo "Windows deployer verification failed with ${failures} finding(s)." >&2
    exit 1
fi
echo "Windows deployer artifact verified: ${EXECUTABLE}"
