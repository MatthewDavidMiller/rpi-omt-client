#!/bin/bash
# Collect a request-correlated, wall-budgeted host diagnostics snapshot.

set -euo pipefail

export LC_ALL=C
umask 027

for obsolete_name in \
    OMT_HOST_DEBUG_OUTPUT \
    OMT_HOST_DEBUG_REQUEST_FILE \
    OMT_HOST_DEBUG_BUDGET_SECONDS \
    OMT_HOST_DEBUG_PCAP_OUTPUT \
    OMT_HOST_DEBUG_PCAP_METADATA_OUTPUT; do
    if [[ -n "${!obsolete_name+x}" ]]; then
        echo "Obsolete ${obsolete_name} is not supported; migrate to OMT_DIAGNOSTICS_*." >&2
        exit 1
    fi
done

OUTPUT_FILE="${OMT_DIAGNOSTICS_HOST_REPORT_FILE:-/var/lib/omt-client/diagnostics/host-report.txt}"
REQUEST_FILE="${OMT_DIAGNOSTICS_HOST_REQUEST_FILE:-/var/lib/omt-client/diagnostics/request}"
INSTALL_DIR="${OMT_INSTALL_DIR:-/opt/omt-client}"
DIAGNOSTICS_BUDGET_SECONDS="${OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS:-25}"
OUTPUT_DIR="$(dirname "${OUTPUT_FILE}")"
PCAP_OUTPUT_FILE="${OMT_DIAGNOSTICS_HOST_PCAP_FILE:-${OUTPUT_DIR}/host-network.pcap}"
PCAP_METADATA_FILE="${OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE:-${OUTPUT_DIR}/host-network-pcap.txt}"
PCAP_MAX_BYTES=67108864
HOST_SECTION_MAX_BYTES=262144
HOST_REQUEST_MAX_BYTES=512

if [[ ! "${DIAGNOSTICS_BUDGET_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
    echo "Invalid OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS: ${DIAGNOSTICS_BUDGET_SECONDS}" >&2
    exit 1
fi

if [[ "${INSTALL_DIR}" == "/" || ! "${INSTALL_DIR}" =~ ^/[A-Za-z0-9._/-]+$ || \
      "${INSTALL_DIR}" == *"//"* || "${INSTALL_DIR}" == */./* || \
      "${INSTALL_DIR}" == */../* || "${INSTALL_DIR}" == */. || \
      "${INSTALL_DIR}" == */.. ]]; then
    echo "Invalid OMT_INSTALL_DIR: ${INSTALL_DIR}" >&2
    exit 1
fi

REQUEST_ID="unknown"
CAPTURE_PCAP=0
if [[ -e "${REQUEST_FILE}" || -L "${REQUEST_FILE}" ]]; then
    REQUEST_ID="invalid"
    request_before=""
    request_after=""
    request_size=""
    request_lines=()
    declare -A request_fields=()
    if [[ -f "${REQUEST_FILE}" && ! -L "${REQUEST_FILE}" ]]; then
        request_before="$(stat -c '%d:%i:%s' -- "${REQUEST_FILE}" 2>/dev/null || true)"
        request_size="${request_before##*:}"
        if [[ "${request_size}" =~ ^[0-9]+$ ]] && \
           (( request_size <= HOST_REQUEST_MAX_BYTES )); then
            mapfile -t request_lines < <(
                head -c "$((HOST_REQUEST_MAX_BYTES + 1))" -- "${REQUEST_FILE}" \
                    2>/dev/null
            )
            request_after="$(stat -c '%d:%i:%s' -- "${REQUEST_FILE}" \
                2>/dev/null || true)"
            if [[ "${request_before}" == "${request_after}" && \
                  -f "${REQUEST_FILE}" && ! -L "${REQUEST_FILE}" ]]; then
                request_valid=true
                for request_line in "${request_lines[@]}"; do
                    request_key="${request_line%%=*}"
                    request_value="${request_line#*=}"
                    if [[ "${request_line}" != *=* || -z "${request_key}" ]]; then
                        request_valid=false
                        break
                    fi
                    if [[ -n "${request_fields[${request_key}]+x}" ]]; then
                        request_valid=false
                        break
                    fi
                    request_fields["${request_key}"]="${request_value}"
                done
                requested_at="${request_fields[requested_at_epoch]:-}"
                now_epoch="$(date +%s)"
                if [[ "${request_valid}" == "true" ]] && \
                   (( ${#request_fields[@]} == 4 )) && \
                   [[ "${request_fields[version]:-}" == "1" ]] && \
                   [[ "${request_fields[request_id]:-}" =~ ^[0-9a-f]{32}$ ]] && \
                   [[ "${request_fields[capture_pcap]:-}" =~ ^[01]$ ]] && \
                   [[ "${requested_at}" =~ ^[0-9]{1,12}$ ]] && \
                   (( 10#${requested_at} <= now_epoch + 5 )) && \
                   (( now_epoch - 10#${requested_at} <= 60 )); then
                    REQUEST_ID="${request_fields[request_id]}"
                    CAPTURE_PCAP="${request_fields[capture_pcap]}"
                fi
            fi
        fi
    fi
fi

mkdir -p "${OUTPUT_DIR}"
chmod 2750 "${OUTPUT_DIR}"
TMP_FILE="$(mktemp "${OUTPUT_FILE}.tmp.XXXXXX")"
BODY_FILE="$(mktemp "${OUTPUT_FILE}.body.XXXXXX")"
MDNS_CAPTURE="$(mktemp "${OUTPUT_FILE}.mdns.XXXXXX")"
OMT_CAPTURE="$(mktemp "${OUTPUT_FILE}.omt.XXXXXX")"
PCAP_METADATA_TMP="$(mktemp "${PCAP_METADATA_FILE}.tmp.XXXXXX")"
PCAP_STAGE_DIR="$(mktemp -d "${OUTPUT_FILE}.pcap.XXXXXX")"
PCAP_CAPTURE_BASE="${PCAP_STAGE_DIR}/capture.pcap"
PCAP_STATS="${PCAP_STAGE_DIR}/tcpdump-stats.txt"
TEXT_CAPTURE_PIDS=()
PCAP_PID=""
PCAP_EXIT_STATUS="not_started"
PCAP_CAPTURE_SECONDS=0
# Set by `run` when the wall budget runs out mid-collection. The container reads
# the header before it reads anything else, so this is what tells an operator
# that missing sections are a budget outcome rather than a broken host.
SECTIONS_SKIPPED=false

cleanup() {
    local pid
    for pid in "${TEXT_CAPTURE_PIDS[@]}"; do
        kill "${pid}" 2>/dev/null || true
        wait "${pid}" 2>/dev/null || true
    done
    if [[ -n "${PCAP_PID}" ]]; then
        kill "${PCAP_PID}" 2>/dev/null || true
        wait "${PCAP_PID}" 2>/dev/null || true
    fi
    rm -f "${TMP_FILE}" "${BODY_FILE}" "${MDNS_CAPTURE}" "${OMT_CAPTURE}" \
        "${PCAP_METADATA_TMP}" "${PCAP_CAPTURE_BASE}" \
        "${PCAP_CAPTURE_BASE}0" "${PCAP_CAPTURE_BASE}1" "${PCAP_STATS}"
    rmdir "${PCAP_STAGE_DIR}" 2>/dev/null || true
}
trap cleanup EXIT
SECONDS=0

section() {
    printf '\n## %s\n' "$1"
}

remaining_seconds() {
    local remaining=$((DIAGNOSTICS_BUDGET_SECONDS - SECONDS))
    (( remaining > 0 )) || remaining=0
    printf '%s\n' "${remaining}"
}

run() {
    local label="$1"
    local remaining command_timeout status section_tmp output_size
    local -a pipeline_status
    shift
    section "${label}"
    remaining="$(remaining_seconds)"
    if (( remaining <= 0 )); then
        SECTIONS_SKIPPED=true
        echo "skipped=host diagnostics budget exhausted"
        return 0
    fi
    command_timeout="${remaining}"
    (( command_timeout <= 8 )) || command_timeout=8
    section_tmp="$(mktemp "${OUTPUT_FILE}.section.XXXXXX")"
    set +e
    timeout "${command_timeout}" "$@" 2>&1 | \
        head -c "$((HOST_SECTION_MAX_BYTES + 1))" > "${section_tmp}"
    pipeline_status=("${PIPESTATUS[@]}")
    set -e
    status="${pipeline_status[0]}"
    output_size="$(wc -c < "${section_tmp}")"
    if (( output_size > HOST_SECTION_MAX_BYTES )); then
        head -c "${HOST_SECTION_MAX_BYTES}" "${section_tmp}"
        printf '\noutput_truncated=yes retained_bytes=%s\n' \
            "${HOST_SECTION_MAX_BYTES}"
        [[ "${status}" -eq 141 ]] && status=0
    else
        cat "${section_tmp}"
    fi
    rm -f -- "${section_tmp}"
    if [[ "${status}" -eq 124 ]]; then
        echo "timed_out_after_seconds=${command_timeout}"
    elif [[ "${status}" -ne 0 ]]; then
        echo "exit_status=${status}"
    fi
}

start_packet_captures() {
    local capture_seconds=$((DIAGNOSTICS_BUDGET_SECONDS - 2))
    (( capture_seconds > 0 )) || capture_seconds=1
    (( capture_seconds <= 20 )) || capture_seconds=20
    if [[ "${CAPTURE_PCAP}" == "0" ]]; then
        PCAP_EXIT_STATUS="disabled"
        rm -f -- "${PCAP_OUTPUT_FILE}"
    fi
    if ! command -v tcpdump >/dev/null 2>&1; then
        printf '%s\n' 'capture_unavailable=tcpdump not installed' > "${MDNS_CAPTURE}"
        printf '%s\n' 'capture_unavailable=tcpdump not installed' > "${OMT_CAPTURE}"
        if [[ "${CAPTURE_PCAP}" == "0" ]]; then
            printf '%s\n' 'raw packet capture was not requested' > "${PCAP_STATS}"
        else
            printf '%s\n' 'tcpdump not installed' > "${PCAP_STATS}"
        fi
        return
    fi

    timeout "${capture_seconds}" tcpdump -ni any -nn -tttt -s 128 \
        'udp port 5353' -c 100 > "${MDNS_CAPTURE}" 2>&1 &
    TEXT_CAPTURE_PIDS+=("$!")
    timeout "${capture_seconds}" tcpdump -ni any -nn -tttt -s 128 \
        'udp port 5353 or tcp portrange 6399-6600 or udp portrange 6399-6600' -c 100 \
        > "${OMT_CAPTURE}" 2>&1 &
    TEXT_CAPTURE_PIDS+=("$!")

    if [[ "${CAPTURE_PCAP}" == "1" ]]; then
        PCAP_CAPTURE_SECONDS="${capture_seconds}"
        timeout --signal=INT --kill-after=2 "${capture_seconds}" \
            tcpdump -i any -n -s 0 -U -C 64 -W 1 -w "${PCAP_CAPTURE_BASE}" \
            2> "${PCAP_STATS}" &
        PCAP_PID="$!"
    else
        PCAP_EXIT_STATUS="disabled"
        printf '%s\n' 'raw packet capture was not requested' > "${PCAP_STATS}"
        rm -f -- "${PCAP_OUTPUT_FILE}"
    fi
}

finish_packet_captures() {
    local pid
    for pid in "${TEXT_CAPTURE_PIDS[@]}"; do
        wait "${pid}" 2>/dev/null || true
    done
    TEXT_CAPTURE_PIDS=()
    if [[ -n "${PCAP_PID}" ]]; then
        if wait "${PCAP_PID}" 2>/dev/null; then
            PCAP_EXIT_STATUS=0
        else
            PCAP_EXIT_STATUS=$?
        fi
        PCAP_PID=""
    fi
}

set_publication_permissions() {
    local path="$1"
    local output_gid
    chmod 640 "${path}"
    if (( EUID == 0 )); then
        output_gid="$(stat -c '%g' "${OUTPUT_DIR}")"
        chown "root:${output_gid}" "${path}"
    fi
}

publish_pcap() {
    local candidate="" capture_status="failed" size_bytes=0
    local sha256 magic_hex="unavailable"
    local possible

    sha256="$(printf '0%.0s' {1..64})"

    for possible in \
        "${PCAP_CAPTURE_BASE}" "${PCAP_CAPTURE_BASE}0" "${PCAP_CAPTURE_BASE}1"; do
        if [[ -f "${possible}" && ! -L "${possible}" ]]; then
            candidate="${possible}"
            break
        fi
    done

    if [[ "${PCAP_EXIT_STATUS}" == "disabled" ]]; then
        capture_status="disabled"
    elif [[ "${PCAP_EXIT_STATUS}" == "not_started" ]]; then
        capture_status="unavailable"
    elif [[ -z "${candidate}" ]]; then
        capture_status="failed"
    else
        size_bytes="$(stat -c '%s' "${candidate}")"
        sha256="$(sha256sum "${candidate}" | awk '{print $1}')"
        magic_hex="$(od -An -N4 -tx1 "${candidate}" | tr -d ' \n')"
        if (( size_bytes > PCAP_MAX_BYTES )); then
            capture_status="oversized"
        elif [[ ! "${magic_hex}" =~ ^(d4c3b2a1|a1b2c3d4|4d3cb2a1|a1b23c4d|0a0d0d0a)$ ]]; then
            capture_status="invalid"
        elif [[ "${PCAP_EXIT_STATUS}" == "124" ]]; then
            capture_status="time_limit"
        elif [[ "${PCAP_EXIT_STATUS}" == "0" ]] && (( size_bytes >= 63000000 )); then
            capture_status="size_limit"
        elif [[ "${PCAP_EXIT_STATUS}" == "0" ]]; then
            capture_status="complete"
        else
            capture_status="failed"
        fi
    fi

    if [[ "${capture_status}" =~ ^(complete|time_limit|size_limit)$ ]]; then
        set_publication_permissions "${candidate}"
        mv -fT "${candidate}" "${PCAP_OUTPUT_FILE}"
        sync -f "${PCAP_OUTPUT_FILE}"
        sync -d "${OUTPUT_DIR}"
    else
        # Nothing publishable this run, so any capture left by an earlier one is
        # now stale. The container refuses to ship it -- it checks capture_status
        # against the metadata it just read -- but an unfiltered capture of the
        # operator's network is not something to leave sitting on the SD card
        # until some later run happens to overwrite it.
        rm -f -- "${PCAP_OUTPUT_FILE}"
        sync -d "${OUTPUT_DIR}"
    fi

    {
        printf 'version=1\n'
        printf 'request_id=%s\n' "${REQUEST_ID}"
        printf 'capture_status=%s\n' "${capture_status}"
        printf 'capture_interface=any\n'
        printf 'capture_filter=none\n'
        printf 'capture_snaplen=full\n'
        printf 'capture_seconds=%s\n' "${PCAP_CAPTURE_SECONDS}"
        printf 'max_bytes=%s\n' "${PCAP_MAX_BYTES}"
        printf 'size_bytes=%s\n' "${size_bytes}"
        printf 'sha256=%s\n' "${sha256}"
        printf 'pcap_magic=%s\n' "${magic_hex}"
        printf 'tcpdump_exit_status=%s\n' "${PCAP_EXIT_STATUS}"
        printf '\ntcpdump_statistics:\n'
        sed -n '1,80p' "${PCAP_STATS}" 2>&1 || true
    } > "${PCAP_METADATA_TMP}"
    set_publication_permissions "${PCAP_METADATA_TMP}"
    mv -fT "${PCAP_METADATA_TMP}" "${PCAP_METADATA_FILE}"
    sync -f "${PCAP_METADATA_FILE}"
    sync -d "${OUTPUT_DIR}"
}

# Start independent samples before Docker/Avahi/provider checks begin. OMT media
# therefore cannot consume the mDNS capture's packet limit.
start_packet_captures

# The body is collected first and the header composed from what it cost. The
# header's `status` is the container's only summary of this run, so it cannot be
# asserted before the run happens: written up front it would claim "complete"
# for a collection the budget cut short.
{
    section "metadata"
    printf 'request_id=%s\n' "${REQUEST_ID}"
    printf 'diagnostics_host_budget_seconds=%s\n' "${DIAGNOSTICS_BUDGET_SECONDS}"
    date -u +timestamp_utc=%Y%m%dT%H%M%SZ
    hostname

    run "Pi model" sh -c \
        'if [ -r /proc/device-tree/model ]; then tr -d "\000" < /proc/device-tree/model; printf "\n"; else echo unavailable; fi'
    run "OS release" cat /etc/os-release
    run "kernel" uname -a
    run "OpenRC runlevels" rc-status --all
    run "effective kernel command line (sanitized)" sh -c \
        'sed -E "s/((password|passwd|secret|token|credential|psk|key)=)[^ ]+/\1<redacted>/Ig" /proc/cmdline'
    run "kernel and SSH hardening" sh -c \
        'sysctl fs.protected_fifos fs.protected_hardlinks fs.protected_regular fs.protected_symlinks fs.suid_dumpable kernel.dmesg_restrict kernel.kptr_restrict kernel.perf_event_paranoid kernel.randomize_va_space kernel.sysrq kernel.unprivileged_bpf_disabled net.core.bpf_jit_harden net.ipv4.conf.all.rp_filter net.ipv4.conf.default.rp_filter net.ipv4.conf.all.arp_ignore net.ipv4.conf.all.arp_announce net.ipv4.conf.all.drop_gratuitous_arp net.ipv4.tcp_fastopen net.ipv6.conf.all.accept_ra net.ipv6.conf.default.accept_ra net.ipv6.conf.all.autoconf net.ipv6.conf.default.autoconf net.ipv6.conf.all.router_solicitations 2>&1; printf "\n### effective SSH policy\n"; sshd -T 2>&1 | grep -E "^(allowgroups|disableforwarding|kbdinteractiveauthentication|maxauthtries|maxsessions|maxstartups|passwordauthentication|permitrootlogin) " || true'
    run "CPU frequency governor" sh -c \
        'for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do [ -r "$g" ] || continue; printf "%s=" "$g"; cat "$g"; done'
    run "Wi-Fi power save" sh -c \
        'if ! command -v iw >/dev/null 2>&1; then echo iw_unavailable; exit 0; fi; iw dev 2>/dev/null || true; for path in /sys/class/net/*/wireless; do [ -d "$path" ] || continue; iface=${path#/sys/class/net/}; iface=${iface%/wireless}; printf "%s " "$iface"; iw dev "$iface" get power_save 2>/dev/null || echo unavailable; done'
    run "managed HDMI settings" sh -c \
        'for file in /media/mmcblk0p1/usercfg.txt /boot/usercfg.txt /etc/omt-client/installer.conf; do [ -e "$file" ] || continue; printf "### %s\n" "$file"; if [ "${file##*/}" = usercfg.txt ]; then sed -n "/^# BEGIN OMT Client HDMI configuration$/,/^# END OMT Client HDMI configuration$/p" "$file" 2>&1; else cat "$file" 2>&1; fi; done'

    run "service definitions (sanitized)" sh -c \
        'for file in /etc/init.d/omt-client* /etc/conf.d/omt-client*; do [ -f "$file" ] || continue; printf "### %s\n" "$file"; sed -E "s/(([A-Za-z0-9_]*(PASSWORD|SECRET|TOKEN|CREDENTIAL|PRIVATE_KEY)[A-Za-z0-9_]*)=)[^[:space:]]+/\1<redacted>/Ig" "$file"; done'
    run "OMT service status (sanitized)" sh -c \
        'rc-service omt-client status 2>&1 | sed -E "s/((password|passwd|secret|token|credential|psk|key)[[:space:]]*[:=][[:space:]]*)[^[:space:]]+/\1<redacted>/Ig"'
    run "OpenRC OMT lifecycle state" rc-status --all
    run "OMT host log" sh -c \
        'tail -n 400 /var/log/messages 2>&1 | grep -E "omt-client|docker|avahi" | sed -E "s/((password|passwd|secret|token|credential|psk|key)[[:space:]]*[:=][[:space:]]*)[^[:space:]]+/\1<redacted>/Ig" || true'
    run "filtered Avahi proxy status" rc-service omt-client-avahi-proxy status
    run "host diagnostics watcher status" rc-service omt-client-host-diagnostics status
    run "reboot watcher status" rc-service omt-client-reboot status

    run "Docker version" docker version
    run "Docker Compose version" docker compose version
    run "target image inspect" docker image inspect --format \
        'id={{.Id}} created={{.Created}} architecture={{.Architecture}} os={{.Os}} version_label={{index .Config.Labels "org.opencontainers.image.version"}}' omt-client
    run "target container inspect" sh -c \
        'cd "$1" && cid=$(docker compose -f deploy/compose.yml ps -q omt-client 2>/dev/null || true); if [ -n "$cid" ]; then docker inspect --format '\''id={{.Id}} image={{.Image}} status={{.State.Status}} pid={{.State.Pid}} restart_count={{.RestartCount}} exit_code={{.State.ExitCode}} oom_killed={{.State.OOMKilled}} state_error={{json .State.Error}} health={{if .State.Health}}{{.State.Health.Status}}{{else}}not_configured{{end}} started_at={{.State.StartedAt}} finished_at={{.State.FinishedAt}} restart_policy={{.HostConfig.RestartPolicy.Name}} restart_maximum_retry_count={{.HostConfig.RestartPolicy.MaximumRetryCount}} memory_limit={{.HostConfig.Memory}} memory_swap={{.HostConfig.MemorySwap}} nano_cpus={{.HostConfig.NanoCpus}} cpu_quota={{.HostConfig.CpuQuota}} cpu_period={{.HostConfig.CpuPeriod}} cpuset_cpus={{.HostConfig.CpusetCpus}} pids_limit={{.HostConfig.PidsLimit}} user={{.Config.User}} network_mode={{.HostConfig.NetworkMode}} readonly_rootfs={{.HostConfig.ReadonlyRootfs}} privileged={{.HostConfig.Privileged}} apparmor={{.AppArmorProfile}} userns_mode={{.HostConfig.UsernsMode}} security_opt={{json .HostConfig.SecurityOpt}} cap_drop={{json .HostConfig.CapDrop}} group_add={{json .HostConfig.GroupAdd}} devices={{json .HostConfig.Devices}} mounts={{json .Mounts}}'\'' "$cid"; else echo no_container; fi' \
        sh "${INSTALL_DIR}"
    run "Docker security and user namespaces" sh -c \
        'docker info --format '\''security_options={{json .SecurityOptions}} rootless={{json .Rootless}} docker_root={{.DockerRootDir}}'\'' 2>&1; printf "kernel_apparmor="; cat /sys/module/apparmor/parameters/enabled 2>&1 || true; printf "userns_clone="; cat /proc/sys/kernel/unprivileged_userns_clone 2>&1 || true'
    run "container resource state" sh -c \
        'cd "$1" && cid=$(docker compose -f deploy/compose.yml ps -q omt-client 2>/dev/null || true); if [ -n "$cid" ]; then docker stats --no-stream --format "name={{.Name}} cpu={{.CPUPerc}} memory={{.MemUsage}} pids={{.PIDs}} network={{.NetIO}} block={{.BlockIO}}" "$cid"; else echo no_container; fi' \
        sh "${INSTALL_DIR}"
    run "host resource state" sh -c \
        'uptime; printf "\n"; free -h; printf "\n"; df -h / /var/lib/docker 2>&1'

    run "device character nodes and GIDs" sh -c \
        'find /dev/dri /dev/snd -maxdepth 1 -type c -exec stat -c "path=%n type=%F uid=%u gid=%g mode=%a" {} + 2>&1'
    run "container supplemental groups versus devices" sh -c \
        'cd "$1" && cid=$(docker compose -f deploy/compose.yml ps -q omt-client 2>/dev/null || true); if [ -n "$cid" ]; then docker exec "$cid" sh -c '\''id; for root in /dev/dri /dev/snd; do find "$root" -maxdepth 1 -type c -exec stat -c "path=%n uid=%u gid=%g mode=%a" {} + 2>/dev/null; done'\''; else echo no_container; fi' \
        sh "${INSTALL_DIR}"
    run "deployed artifact hashes" sh -c \
        'cd "$1" || exit; for file in deploy/compose.yml deploy/host/install.sh deploy/host/uninstall.sh deploy/host/host-diagnostics.sh omt-client-arm64.tar.gz; do if [ -f "$file" ]; then sha256sum "$file"; else printf "missing=%s\n" "$file"; fi; done' \
        sh "${INSTALL_DIR}"

    run "network interfaces and counters" ip -details -statistics link show
    run "network addresses and counters" ip -statistics address show
    run "routes (all tables)" ip route show table all
    run "policy routing rules" ip rule show
    run "link counters from proc" cat /proc/net/dev
    run "kernel network counters" sh -c \
        'if command -v nstat >/dev/null 2>&1; then nstat -az; else echo nstat_not_installed; fi'
    run "interface driver counters" sh -c \
        'if ! command -v ethtool >/dev/null 2>&1; then echo ethtool_not_installed; exit; fi; for path in /sys/class/net/*; do iface=${path##*/}; printf "### %s\n" "$iface"; ethtool -S "$iface" 2>&1 | sed -n "1,200p"; done'
    run "interface offload and ring settings" sh -c \
        'if ! command -v ethtool >/dev/null 2>&1; then echo ethtool_not_installed; exit; fi; for path in /sys/class/net/*; do iface=${path##*/}; [ "$iface" = lo ] && continue; printf "### %s offload\n" "$iface"; ethtool -k "$iface" 2>&1 | sed -n "1,240p"; printf "### %s rings\n" "$iface"; ethtool -g "$iface" 2>&1 | sed -n "1,160p"; done'
    run "Wi-Fi association signal and rates" sh -c \
        'if ! command -v iw >/dev/null 2>&1; then echo iw_not_installed; exit; fi; iw dev 2>&1; iw dev 2>/dev/null | while read -r key iface remainder; do [ "$key" = Interface ] || continue; printf "### %s link\n" "$iface"; iw dev "$iface" link 2>&1; printf "### %s stations\n" "$iface"; iw dev "$iface" station dump 2>&1 | sed -n "1,240p"; done'
    run "radio block state" sh -c \
        'if command -v rfkill >/dev/null 2>&1; then rfkill list; else echo rfkill_not_installed; fi'
    run "neighbor tables" ip -details neigh show
    run "bridge links FDB and MDB" sh -c \
        'if ! command -v bridge >/dev/null 2>&1; then echo bridge_not_installed; exit; fi; bridge -details link show 2>&1; printf "\n### FDB\n"; bridge -details fdb show 2>&1; printf "\n### MDB\n"; bridge -details mdb show 2>&1'
    run "traffic-control queues" sh -c \
        'if ! command -v tc >/dev/null 2>&1; then echo tc_not_installed; exit; fi; tc -s qdisc show 2>&1; printf "\n### classes\n"; tc -s class show 2>&1'
    run "multicast addresses" ip maddr
    run "IGMP memberships" cat /proc/net/igmp
    run "IGMP configuration and multicast routing" sh -c \
        'sysctl net.ipv4.conf.all.mc_forwarding net.ipv4.conf.all.rp_filter net.ipv4.conf.default.rp_filter 2>&1; ip mroute show 2>&1 || true'
    run "UDP sockets" ss -ulpn
    run "TCP and UDP socket state" ss -tulpnea
    run "socket summary" ss -s
    run "mDNS listeners" sh -c "ss -ulpn | grep -E '(:5353|mdns)' || true"
    run "host Avahi OMT records" sh -c \
        "command -v avahi-browse >/dev/null 2>&1 && avahi-browse --parsable --resolve --terminate _omt._tcp 2>&1 || echo 'avahi-browse not installed'"
    run "real and filtered D-Bus socket metadata" sh -c \
        'for socket in /run/dbus/system_bus_socket /run/avahi-daemon/socket /var/lib/omt-client/avahi/system-bus; do printf "### %s\n" "$socket"; stat -Lc "type=%F uid=%u gid=%g mode=%a size=%s" "$socket" 2>&1; done'
    run "container filtered D-Bus check" sh -c \
        'cd "$1" && cid=$(docker compose -f deploy/compose.yml ps -q omt-client 2>/dev/null || true); if [ -z "$cid" ]; then echo no_container; exit; fi; docker exec "$cid" sh -c '\''printf "DBUS_SYSTEM_BUS_ADDRESS=%s\n" "${DBUS_SYSTEM_BUS_ADDRESS:-<unset>}"; stat -Lc "socket_type=%F uid=%u gid=%g mode=%a" /host-avahi/system-bus 2>&1; timeout 5 dbus-send --bus="${DBUS_SYSTEM_BUS_ADDRESS}" --type=method_call --print-reply --dest=org.freedesktop.Avahi / org.freedesktop.Avahi.Server.GetVersionString 2>&1'\''' \
        sh "${INSTALL_DIR}"
    run "resolver configuration" cat /etc/resolv.conf
    run "wpa_supplicant status" sh -c \
        'for interface in /run/wpa_supplicant/*; do [ -S "$interface" ] || continue; wpa_cli -i "${interface##*/}" status 2>&1 | sed -E "s/^(psk|password)=.*/\1=<redacted>/"; done'
    run "Avahi status" rc-service avahi-daemon status
    run "D-Bus and Avahi log" sh -c \
        'tail -n 400 /var/log/messages 2>&1 | grep -E "dbus|avahi" || true'
    run "nftables rules" nft list ruleset
    run "iptables input rules" iptables -S INPUT
    # shellcheck disable=SC2016 # Expanded by the nested host shell.
    run "DRM cards, drivers, and connectors" sh -c \
        'for card in /sys/class/drm/card[0-9]*; do [ -e "$card" ] || continue; printf "### %s\n" "$card"; printf "driver="; basename "$(readlink -f "$card/device/driver" 2>/dev/null)" 2>/dev/null || echo unknown; done; for connector in /sys/class/drm/card*-*; do [ -e "$connector/status" ] || continue; printf "### %s\n" "$connector"; printf "connector_id="; cat "$connector/connector_id" 2>&1; printf "status="; cat "$connector/status" 2>&1; printf "edid_bytes="; wc -c < "$connector/edid" 2>/dev/null || echo unavailable; sed -n "1,20p" "$connector/modes" 2>&1; done'
    run "host vc4 modetest" sh -c \
        'if command -v modetest >/dev/null 2>&1; then timeout 5 modetest -M vc4 2>&1 | sed -n "1,400p"; else echo modetest_not_installed; fi'
    run "host ALSA HDMI state" sh -c \
        'for file in /proc/asound/cards /proc/asound/pcm /proc/asound/devices; do printf "### %s\n" "$file"; cat "$file" 2>&1; done; for eld in /proc/asound/card*/eld*; do [ -e "$eld" ] || continue; printf "### %s\n" "$eld"; sed -n "1,120p" "$eld"; done'
    run "kernel security denials" sh -c \
        'dmesg 2>&1 | grep -Ei "apparmor|audit|denied|seccomp|operation not permitted" | tail -n 200 || true'
    run "kernel vc4 DRM HDMI and ALSA messages" sh -c \
        'dmesg 2>&1 | grep -Ei "vc4|drm|hdmi|edid|alsa|snd|audio" | tail -n 300 || true'

    finish_packet_captures
    publish_pcap
    section "unfiltered host PCAP"
    cat "${PCAP_METADATA_FILE}"
    section "mDNS packet capture (independent sample)"
    cat "${MDNS_CAPTURE}"
    section "OMT transport packet capture (independent sample)"
    cat "${OMT_CAPTURE}"

    section "budget summary"
    printf 'elapsed_seconds=%s\n' "${SECONDS}"
    printf 'budget_exhausted=%s\n' "$(( SECONDS >= DIAGNOSTICS_BUDGET_SECONDS ))"
} > "${BODY_FILE}"

REPORT_STATUS=complete
if [[ "${SECTIONS_SKIPPED}" == "true" ]]; then
    REPORT_STATUS=partial
fi
{
    printf 'version=1\n'
    printf 'request_id=%s\n' "${REQUEST_ID}"
    printf 'status=%s\n' "${REPORT_STATUS}"
    cat "${BODY_FILE}"
} > "${TMP_FILE}"

chmod 640 "${TMP_FILE}"
set_publication_permissions "${TMP_FILE}"
mv -fT "${TMP_FILE}" "${OUTPUT_FILE}"
sync -f "${OUTPUT_FILE}"
sync -d "${OUTPUT_DIR}"
