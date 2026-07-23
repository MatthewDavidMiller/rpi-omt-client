#!/bin/bash
# Unit tests for request tagging and the host diagnostics wall-clock budget.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
HOST_DEBUG="${PROJECT_ROOT}/host-debug.sh"

TEST_DIR="$(mktemp -d)"
trap 'rm -rf "${TEST_DIR}"' EXIT
mkdir -p "${TEST_DIR}/bin" "${TEST_DIR}/install"

cat > "${TEST_DIR}/bin/ip" <<'EOF'
#!/bin/bash
/bin/sleep 5
EOF
chmod +x "${TEST_DIR}/bin/ip"

cat > "${TEST_DIR}/bin/tcpdump" <<'EOF'
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
    printf '0 packets captured\n0 packets received by filter\n0 packets dropped by kernel\n' >&2
else
    printf 'bounded text packet sample\n'
fi
EOF
chmod +x "${TEST_DIR}/bin/tcpdump"

REQUEST_FILE="${TEST_DIR}/request"
OUTPUT_FILE="${TEST_DIR}/host-debug.txt"
PCAP_FILE="${TEST_DIR}/host-network.pcap"
PCAP_METADATA_FILE="${TEST_DIR}/host-network-pcap.txt"
printf 'request_id=request-abc-123\nbudget_seconds=9999\n' > "${REQUEST_FILE}"

PATH="${TEST_DIR}/bin:${PATH}" \
OMT_HOST_DEBUG_OUTPUT="${OUTPUT_FILE}" \
OMT_HOST_DEBUG_REQUEST_FILE="${REQUEST_FILE}" \
OMT_HOST_DEBUG_BUDGET_SECONDS=1 \
OMT_INSTALL_DIR="${TEST_DIR}/install" \
    "${HOST_DEBUG}"

grep -qx 'request_id=request-abc-123' "${OUTPUT_FILE}"
grep -qx 'host_debug_budget_seconds=1' "${OUTPUT_FILE}"
grep -q 'skipped=host diagnostics budget exhausted' "${OUTPUT_FILE}"
grep -q '^## Pi model$' "${OUTPUT_FILE}"
grep -q '^## service definitions (sanitized)$' "${OUTPUT_FILE}"
grep -q '^## OMT service status (sanitized)$' "${OUTPUT_FILE}"
grep -q '^## systemd lifecycle state$' "${OUTPUT_FILE}"
grep -q '^## OMT service journal$' "${OUTPUT_FILE}"
grep -q '^## filtered Avahi proxy status$' "${OUTPUT_FILE}"
grep -q '^## filtered Avahi proxy journal$' "${OUTPUT_FILE}"
grep -q '^## target image inspect$' "${OUTPUT_FILE}"
grep -q '^## Docker security and user namespaces$' "${OUTPUT_FILE}"
grep -q '^## device character nodes and GIDs$' "${OUTPUT_FILE}"
grep -q '^## container supplemental groups versus devices$' "${OUTPUT_FILE}"
grep -q '^## deployed artifact hashes$' "${OUTPUT_FILE}"
grep -q '^## network interfaces and counters$' "${OUTPUT_FILE}"
grep -q '^## interface offload and ring settings$' "${OUTPUT_FILE}"
grep -q '^## Wi-Fi association signal and rates$' "${OUTPUT_FILE}"
grep -q '^## radio block state$' "${OUTPUT_FILE}"
grep -q '^## neighbor tables$' "${OUTPUT_FILE}"
grep -q '^## bridge links FDB and MDB$' "${OUTPUT_FILE}"
grep -q '^## traffic-control queues$' "${OUTPUT_FILE}"
grep -q '^## TCP and UDP socket state$' "${OUTPUT_FILE}"
grep -q '^## real and filtered D-Bus socket metadata$' "${OUTPUT_FILE}"
grep -q '^## container filtered D-Bus check$' "${OUTPUT_FILE}"
grep -q '^## D-Bus journal$' "${OUTPUT_FILE}"
grep -q '^## Avahi journal$' "${OUTPUT_FILE}"
grep -q '^## DRM cards, drivers, and connectors$' "${OUTPUT_FILE}"
grep -q '^## host vc4 modetest$' "${OUTPUT_FILE}"
grep -q '^## host ALSA HDMI state$' "${OUTPUT_FILE}"
grep -q '^## kernel security denials$' "${OUTPUT_FILE}"
grep -q '^## kernel vc4 DRM HDMI and ALSA messages$' "${OUTPUT_FILE}"
grep -q '^## unfiltered host PCAP$' "${OUTPUT_FILE}"
grep -q '^## mDNS packet capture (independent sample)$' "${OUTPUT_FILE}"
grep -q '^## OMT transport packet capture (independent sample)$' "${OUTPUT_FILE}"
[[ "$(stat -c '%a' "${TEST_DIR}")" == "2750" ]]
[[ "$(stat -c '%a' "${OUTPUT_FILE}")" == "640" ]]
[[ "$(stat -c '%a' "${PCAP_FILE}")" == "640" ]]
[[ "$(stat -c '%a' "${PCAP_METADATA_FILE}")" == "640" ]]
[[ "$(stat -c '%s' "${PCAP_FILE}")" == "24" ]]
grep -qx 'request_id=request-abc-123' "${PCAP_METADATA_FILE}"
grep -qx 'capture_status=complete' "${PCAP_METADATA_FILE}"
grep -qx 'capture_interface=any' "${PCAP_METADATA_FILE}"
grep -qx 'capture_filter=none' "${PCAP_METADATA_FILE}"
grep -qx 'capture_snaplen=full' "${PCAP_METADATA_FILE}"
grep -qx 'max_bytes=67108864' "${PCAP_METADATA_FILE}"
grep -qx 'size_bytes=24' "${PCAP_METADATA_FILE}"
grep -qx "sha256=$(sha256sum "${PCAP_FILE}" | awk '{print $1}')" \
    "${PCAP_METADATA_FILE}"
grep -q '^tcpdump_statistics:$' "${PCAP_METADATA_FILE}"
grep -q '^0 packets dropped by kernel$' "${PCAP_METADATA_FILE}"
cmp -s <(head -c 4 "${PCAP_FILE}") <(printf '\324\303\262\241')
grep -q "'udp port 5353' -c 100" "${HOST_DEBUG}"
grep -q "'udp port 5353 or tcp portrange 6399-6600 or udp portrange 6399-6600' -c 100" \
    "${HOST_DEBUG}"
grep -q 'timeout --signal=INT --kill-after=2' "${HOST_DEBUG}"
grep -q 'tcpdump -i any -n -s 0 -U -C 64 -W 1 -w' "${HOST_DEBUG}"
! grep -q 'docker logs' "${HOST_DEBUG}"
! grep -q 'vars.yml' "${HOST_DEBUG}"
grep -q 'restart_count={{.RestartCount}}' "${HOST_DEBUG}"
grep -q 'exit_code={{.State.ExitCode}}' "${HOST_DEBUG}"
grep -q 'oom_killed={{.State.OOMKilled}}' "${HOST_DEBUG}"
grep -q 'restart_policy={{.HostConfig.RestartPolicy.Name}}' "${HOST_DEBUG}"
grep -q 'memory_limit={{.HostConfig.Memory}}' "${HOST_DEBUG}"

assert_invalid_request() {
    local label="$1"
    local request_path="$2"
    local case_output="${TEST_DIR}/host-debug-${label}.txt"
    local case_pcap="${TEST_DIR}/host-network-${label}.pcap"
    local case_metadata="${TEST_DIR}/host-network-${label}.txt"

    PATH="${TEST_DIR}/bin:${PATH}" \
    OMT_HOST_DEBUG_OUTPUT="${case_output}" \
    OMT_HOST_DEBUG_REQUEST_FILE="${request_path}" \
    OMT_HOST_DEBUG_PCAP_OUTPUT="${case_pcap}" \
    OMT_HOST_DEBUG_PCAP_METADATA_OUTPUT="${case_metadata}" \
    OMT_HOST_DEBUG_BUDGET_SECONDS=1 \
    OMT_INSTALL_DIR="${TEST_DIR}/install" \
        "${HOST_DEBUG}" >/dev/null

    grep -qx 'request_id=invalid' "${case_output}"
    [[ "$(stat -c '%a' "${case_output}")" == "640" ]]
}

OVERSIZED_REQUEST="${TEST_DIR}/request-oversized"
head -c 4097 /dev/zero | tr '\0' x > "${OVERSIZED_REQUEST}"
assert_invalid_request "oversized-request" "${OVERSIZED_REQUEST}"

DUPLICATE_REQUEST="${TEST_DIR}/request-duplicate"
printf 'request_id=first\nrequest_id=second\nbudget_seconds=1\n' \
    > "${DUPLICATE_REQUEST}"
assert_invalid_request "duplicate-request" "${DUPLICATE_REQUEST}"

REQUEST_TARGET="${TEST_DIR}/request-target"
printf 'request_id=must-not-follow\nbudget_seconds=1\n' > "${REQUEST_TARGET}"
REQUEST_SYMLINK="${TEST_DIR}/request-symlink"
ln -s "${REQUEST_TARGET}" "${REQUEST_SYMLINK}"
assert_invalid_request "symlink-request" "${REQUEST_SYMLINK}"
grep -qx 'request_id=must-not-follow' "${REQUEST_TARGET}"
grep -q '^HOST_REQUEST_MAX_BYTES=4096$' "${HOST_DEBUG}"

cat > "${TEST_DIR}/bin/cat" <<'EOF'
#!/bin/bash
if [[ "${1:-}" == "/etc/os-release" ]]; then
    head -c 300000 /dev/zero | tr '\0' x
    exit 0
fi
exec /bin/cat "$@"
EOF
chmod +x "${TEST_DIR}/bin/cat"
TRUNCATED_OUTPUT_FILE="${TEST_DIR}/host-debug-truncated.txt"
PATH="${TEST_DIR}/bin:${PATH}" \
OMT_HOST_DEBUG_OUTPUT="${TRUNCATED_OUTPUT_FILE}" \
OMT_HOST_DEBUG_REQUEST_FILE="${REQUEST_FILE}" \
OMT_HOST_DEBUG_BUDGET_SECONDS=1 \
OMT_INSTALL_DIR="${TEST_DIR}/install" \
    "${HOST_DEBUG}"
grep -q '^output_truncated=yes retained_bytes=262144$' \
    "${TRUNCATED_OUTPUT_FILE}"
if (( $(stat -c '%s' "${TRUNCATED_OUTPUT_FILE}") > 16777216 )); then
    echo "FAIL: bounded host diagnostics exceeded the application limit" >&2
    exit 1
fi

echo "PASS: host diagnostics are request-tagged, bounded, and private"
