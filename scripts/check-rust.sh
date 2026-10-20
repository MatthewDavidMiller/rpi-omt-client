#!/bin/bash
# The Rust tree's static gates: formatting, lints on every shipped target, the
# no-third-party-crates rule, the unsafe allowlist, the foreign declarations
# against the platform headers, and the generated sources.
#
#   ./scripts/check-rust.sh
#
# Runs inside the toolbox (scripts/lint.sh calls it). It builds nothing it
# does not have to: clippy checks the aarch64 and Windows targets without
# linking, so no sysroot or mingw link step is involved here.
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

for tool in cargo rustfmt python3; do
    command -v "${tool}" >/dev/null || { echo "ERROR: ${tool} is required" >&2; exit 1; }
done

echo "== rustfmt"
cargo fmt --all -- --check

# No crate from outside the workspace, ever: every package in Cargo.lock must
# be a workspace member, which means none of them carries a `source`.
echo "== workspace-only dependency graph"
cargo generate-lockfile --offline >/dev/null 2>&1 || true
if grep -q '^source = ' Cargo.lock; then
    echo "ERROR: Cargo.lock names a package from outside the workspace:" >&2
    grep -B2 '^source = ' Cargo.lock >&2
    exit 1
fi
members="$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.lock | sort)"
# The [package] name only; [[bin]] tables carry names of their own.
expected="$(find crates -mindepth 2 -maxdepth 2 -name Cargo.toml -exec \
    awk -F'"' '/^\[/ { in_package = ($0 == "[package]") } in_package && /^name = / { print $2 }' {} \; |
    sort)"
if [[ "${members}" != "${expected}" ]]; then
    echo "ERROR: Cargo.lock packages differ from the workspace members" >&2
    diff <(echo "${expected}") <(echo "${members}") >&2 || true
    exit 1
fi

# `unsafe` is confined to the crates that cannot avoid it: the foreign
# declarations, the decoder's kernels and shared output rows, and the fuzz
# entry points. Every other crate forbids it at its root.
echo "== unsafe allowlist"
UNSAFE_CRATES=(omt-sys omt-vmx omt-fuzz)
for manifest in crates/*/Cargo.toml; do
    dir="$(dirname "${manifest}")"
    name="$(basename "${dir}")"
    allowed=false
    for a in "${UNSAFE_CRATES[@]}"; do [[ "${name}" == "${a}" ]] && allowed=true; done
    if [[ "${allowed}" == false ]]; then
        for root in "${dir}/src/lib.rs" "${dir}/src/main.rs"; do
            [[ -f "${root}" ]] || continue
            grep -q '^#!\[forbid(unsafe_code)\]' "${root}" || {
                echo "ERROR: ${root} must forbid unsafe_code" >&2
                exit 1
            }
        done
    fi
done

echo "== clippy (host, aarch64, windows)"
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo clippy --workspace --lib --bins --offline --locked --target aarch64-unknown-linux-musl -- -D warnings
# The Windows deployer's crates only; the appliance crates are Linux-only.
cargo clippy --offline --locked --target x86_64-pc-windows-gnu --all-targets \
    -p omt-sys -p omt-common -p omt-crypto -p omt-deploy -p omt-capsule \
    -p rpi-omt-deploy -p rpi-omt-deploy-gui -- -D warnings

# Every hand-written declaration in omt-sys against the real headers, for
# each shipped target; compiled, never run.
echo "== ABI probe"
python3 tests/abi/check_abi.py

echo "== generated sources"
python3 tools/gen/gen_nfc_tables.py --check
python3 tools/gen/gen_vmx_tables.py --check
python3 tools/gen/gen_templates.py --check
python3 tools/gen/gen_blowfish_tables.py --check

echo "Rust static gates passed."
