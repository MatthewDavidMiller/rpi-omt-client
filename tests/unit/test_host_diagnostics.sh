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

run_case() {
    local label="$1" request_id="$2" capture="$3"
    local request="${CASE_DIR}/diagnostics/request"
    local report="${CASE_DIR}/diagnostics/report-${label}.txt"
    local pcap="${CASE_DIR}/diagnostics/capture-${label}.pcap"
    local metadata="${CASE_DIR}/diagnostics/metadata-${label}.txt"
    printf 'version=1\nrequest_id=%s\ncapture_pcap=%s\nrequested_at_epoch=%s\n' \
        "${request_id}" "${capture}" "$(date +%s)" > "${request}"
    chmod 0600 "${request}"
    PATH="${CASE_DIR}/bin:${PATH}" \
    OMT_DIAGNOSTICS_HOST_REPORT_FILE="${report}" \
    OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${request}" \
    OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS=1 \
    OMT_DIAGNOSTICS_HOST_PCAP_FILE="${pcap}" \
    OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE="${metadata}" \
    OMT_INSTALL_DIR="${CASE_DIR}/install" \
        "${HOST_DIAGNOSTICS}"
    grep -qx 'version=1' "${report}"
    grep -qx "request_id=${request_id}" "${report}"
    grep -qx 'status=complete' "${report}"
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

invalid_request="${CASE_DIR}/diagnostics/request"
printf 'version=1\nrequest_id=wrong\ncapture_pcap=1\nrequested_at_epoch=0\n' \
    > "${invalid_request}"
PATH="${CASE_DIR}/bin:${PATH}" \
OMT_DIAGNOSTICS_HOST_REPORT_FILE="${CASE_DIR}/diagnostics/report-invalid.txt" \
OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${invalid_request}" \
OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS=1 \
OMT_DIAGNOSTICS_HOST_PCAP_FILE="${CASE_DIR}/diagnostics/capture-invalid.pcap" \
OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE="${CASE_DIR}/diagnostics/metadata-invalid.txt" \
OMT_INSTALL_DIR="${CASE_DIR}/install" \
    "${HOST_DIAGNOSTICS}"
grep -qx 'request_id=invalid' "${CASE_DIR}/diagnostics/report-invalid.txt"
[[ ! -e "${CASE_DIR}/diagnostics/capture-invalid.pcap" ]]

if OMT_HOST_DEBUG_OUTPUT="${CASE_DIR}/legacy" "${HOST_DIAGNOSTICS}" \
    >"${CASE_DIR}/obsolete.out" 2>&1; then
    echo "obsolete debug setting was accepted" >&2
    exit 1
fi
grep -q 'migrate to OMT_DIAGNOSTICS_' "${CASE_DIR}/obsolete.out"

echo "Host diagnostics behavior tests passed"
