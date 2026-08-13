#!/bin/bash
# Behavior tests for request-correlated host diagnostics and PCAP opt-in.

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
HOST_DIAGNOSTICS="${ROOT}/deploy/host/host-diagnostics.sh"
CASE_DIR="$(mktemp -d)"
trap 'rm -rf "${CASE_DIR}"' EXIT
mkdir -p "${CASE_DIR}/bin" "${CASE_DIR}/install" "${CASE_DIR}/diagnostics"

cat > "${CASE_DIR}/bin/ip" <<'EOF'
#!/bin/bash
/bin/sleep 5
EOF
chmod 0755 "${CASE_DIR}/bin/ip"

cat > "${CASE_DIR}/bin/tcpdump" <<'EOF'
#!/bin/bash
set -euo pipefail
output=""
previous=""
for argument in "$@"; do
    if [[ "${previous}" == "-w" ]]; then
        output="${argument}"
    fi
    previous="${argument}"
done
if [[ -n "${output}" ]]; then
    # Minimal little-endian classic-PCAP global header.
    printf '\324\303\262\241\002\000\004\000\000\000\000\000\000\000\000\000\377\377\000\000\161\000\000\000' > "${output}"
    printf '0 packets captured\n' >&2
else
    printf 'bounded filtered packet sample\n'
fi
EOF
chmod 0755 "${CASE_DIR}/bin/tcpdump"

# Run one collection against per-label output paths. Every caller needs the same
# eight environment overrides, so they live here rather than in each case. The
# budget stays at one second unless a case asks for more: the `ip` stub above
# outlasts it, so these runs deliberately exercise a curtailed collection.
run_diagnostics() {
    local label="$1" budget="${2:-1}"
    PATH="${CASE_DIR}/bin:${PATH}" \
    OMT_DIAGNOSTICS_HOST_REPORT_FILE="${CASE_DIR}/diagnostics/report-${label}.txt" \
    OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${CASE_DIR}/diagnostics/request" \
    OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS="${budget}" \
    OMT_DIAGNOSTICS_HOST_PCAP_FILE="${CASE_DIR}/diagnostics/capture-${label}.pcap" \
    OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE="${CASE_DIR}/diagnostics/metadata-${label}.txt" \
    OMT_INSTALL_DIR="${CASE_DIR}/install" \
        "${HOST_DIAGNOSTICS}"
}

write_request() {
    local request_id="$1" capture="$2" epoch="${3:-$(date +%s)}"
    printf 'version=1\nrequest_id=%s\ncapture_pcap=%s\nrequested_at_epoch=%s\n' \
        "${request_id}" "${capture}" "${epoch}" \
        > "${CASE_DIR}/diagnostics/request"
    chmod 0600 "${CASE_DIR}/diagnostics/request"
}

run_case() {
    local label="$1" request_id="$2" capture="$3" epoch="${4:-$(date +%s)}"
    local report="${CASE_DIR}/diagnostics/report-${label}.txt"
    local metadata="${CASE_DIR}/diagnostics/metadata-${label}.txt"
    write_request "${request_id}" "${capture}" "${epoch}"
    run_diagnostics "${label}"
    grep -qx 'version=1' "${report}"
    grep -qx "request_id=${request_id}" "${report}"
    # These runs cannot finish inside their one-second budget, and the header is
    # the container's only summary of the run. A fixed "complete" written before
    # collection would tell an operator staring at a report of skipped sections
    # that nothing was missing.
    grep -qx 'status=partial' "${report}"
    grep -q 'skipped=host diagnostics budget exhausted' "${report}"
    grep -qx "request_id=${request_id}" "${metadata}"
    grep -qx 'max_bytes=67108864' "${metadata}"
}

unchecked_id=11111111111111111111111111111111
stale_pcap="${CASE_DIR}/diagnostics/capture-unchecked.pcap"
printf 'stale' > "${stale_pcap}"
run_case unchecked "${unchecked_id}" 0
grep -qx 'capture_status=disabled' \
    "${CASE_DIR}/diagnostics/metadata-unchecked.txt"
[[ ! -e "${stale_pcap}" ]]

checked_id=22222222222222222222222222222222
run_case checked "${checked_id}" 1
grep -Eq '^capture_status=(complete|time_limit|size_limit)$' \
    "${CASE_DIR}/diagnostics/metadata-checked.txt"
[[ -s "${CASE_DIR}/diagnostics/capture-checked.pcap" ]]
grep -qx 'pcap_magic=d4c3b2a1' \
    "${CASE_DIR}/diagnostics/metadata-checked.txt"

# The accepted timestamp shape allows leading zeros, so the freshness
# comparison has to be base 10. Read as octal, a zero-padded epoch is an
# arithmetic error, the request is discarded, and the operator gets a report
# that never correlates with what they asked for.
padded_id=33333333333333333333333333333333
run_case padded "${padded_id}" 0 "$(printf '%012d' "$(date +%s)")"

# A request older than the freshness window is a leftover, not this collection.
run_case_rejected() {
    local label="$1"
    write_request "$2" "$3" "$4"
    run_diagnostics "${label}"
    grep -qx 'request_id=invalid' "${CASE_DIR}/diagnostics/report-${label}.txt"
    [[ ! -e "${CASE_DIR}/diagnostics/capture-${label}.pcap" ]]
}

run_case_rejected stale 44444444444444444444444444444444 1 1
run_case_rejected future 55555555555555555555555555555555 1 "$(($(date +%s) + 600))"
run_case_rejected invalid wrong 1 0

# A capture was requested but produced nothing publishable. The capture an
# earlier run left behind describes a different moment and no longer matches the
# metadata published beside it, so it must not survive this run: the container
# refuses to ship it either way, but an unfiltered capture of the operator's
# network should not linger on the SD card waiting to be overwritten.
failed_id=66666666666666666666666666666666
retained_pcap="${CASE_DIR}/diagnostics/capture-failed.pcap"
printf 'previous capture' > "${retained_pcap}"
cat > "${CASE_DIR}/bin/tcpdump" <<'EOF'
#!/bin/bash
printf 'tcpdump: no suitable device found\n' >&2
exit 1
EOF
chmod 0755 "${CASE_DIR}/bin/tcpdump"
write_request "${failed_id}" 1
run_diagnostics failed
grep -qx "request_id=${failed_id}" "${CASE_DIR}/diagnostics/metadata-failed.txt"
grep -Eq '^capture_status=(failed|unavailable|invalid)$' \
    "${CASE_DIR}/diagnostics/metadata-failed.txt"
[[ ! -e "${retained_pcap}" ]]

# A collection that skipped nothing is the only one allowed to say so. The
# container accepts "complete" and "partial" and nothing else, so both halves of
# that contract need a producer.
cat > "${CASE_DIR}/bin/ip" <<'EOF'
#!/bin/bash
printf 'link/ether 02:00:00:00:00:01\n'
EOF
chmod 0755 "${CASE_DIR}/bin/ip"
cat > "${CASE_DIR}/bin/tcpdump" <<'EOF'
#!/bin/bash
printf 'bounded filtered packet sample\n'
EOF
chmod 0755 "${CASE_DIR}/bin/tcpdump"

complete_id=77777777777777777777777777777777
write_request "${complete_id}" 0
run_diagnostics complete 120
grep -qx "request_id=${complete_id}" "${CASE_DIR}/diagnostics/report-complete.txt"
grep -qx 'status=complete' "${CASE_DIR}/diagnostics/report-complete.txt"
grep -qx '## kernel and SSH hardening' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -q 'net.core.bpf_jit_harden' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -q 'net.ipv4.conf.all.rp_filter' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -q 'net.ipv6.conf.all.accept_ra' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -q 'net.ipv6.conf.all.autoconf' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -q 'net.ipv4.conf.all.arp_ignore' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -qx '## CPU frequency governor' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -qx '## Wi-Fi power save' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
grep -q '### effective SSH policy' \
    "${CASE_DIR}/diagnostics/report-complete.txt"
if grep -q 'skipped=host diagnostics budget exhausted' \
    "${CASE_DIR}/diagnostics/report-complete.txt"; then
    echo "a report claiming completeness skipped sections" >&2
    exit 1
fi

if OMT_HOST_DEBUG_OUTPUT="${CASE_DIR}/legacy" "${HOST_DIAGNOSTICS}" \
    >"${CASE_DIR}/obsolete.out" 2>&1; then
    echo "obsolete debug setting was accepted" >&2
    exit 1
fi
grep -q 'migrate to OMT_DIAGNOSTICS_' "${CASE_DIR}/obsolete.out"

echo "Host diagnostics behavior tests passed"
