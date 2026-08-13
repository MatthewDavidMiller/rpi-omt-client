#!/bin/bash
# Idempotently allow or remove source-scoped OMT sender traffic.
# Supports firewalld (workstation) and nftables (Alpine appliance hosts).
set -euo pipefail

usage() {
    echo "Usage: $0 {allow|remove|status} SOURCE_IPV4_OR_CIDR" >&2
}

[[ $# -eq 2 ]] || {
    usage
    exit 2
}
action="$1"
source_input="$2"
zone="${OMT_TEST_SENDER_FIREWALL_ZONE:-public}"
[[ "${zone}" =~ ^[A-Za-z0-9_-]+$ ]] || {
    echo "ERROR: invalid firewalld zone" >&2
    exit 2
}

valid_ipv4() {
    local ip="$1"
    local o1 o2 o3 o4 rest
    IFS=. read -r o1 o2 o3 o4 rest <<EOF
${ip}
EOF
    [[ -z "${rest:-}" ]] || return 1
    [[ "${o1:-}" =~ ^[0-9]+$ && "${o2:-}" =~ ^[0-9]+$ && \
       "${o3:-}" =~ ^[0-9]+$ && "${o4:-}" =~ ^[0-9]+$ ]] || return 1
    (( o1 <= 255 && o2 <= 255 && o3 <= 255 && o4 <= 255 )) || return 1
    # Leading zeros would make this look like octal in some parsers.
    [[ "${o1}${o2}${o3}${o4}" =~ ^[0-9]+$ ]] || return 1
    [[ ! "${o1}" =~ ^0[0-9] ]] || return 1
    [[ ! "${o2}" =~ ^0[0-9] ]] || return 1
    [[ ! "${o3}" =~ ^0[0-9] ]] || return 1
    [[ ! "${o4}" =~ ^0[0-9] ]] || return 1
}

if [[ "${source_input}" == */* ]]; then
    source_ip="${source_input%/*}"
    source_prefix="${source_input#*/}"
else
    source_ip="${source_input}"
    source_prefix=32
fi
valid_ipv4 "${source_ip}" || {
    echo "ERROR: invalid source IPv4 address or CIDR: ${source_input}" >&2
    exit 2
}
[[ "${source_prefix}" =~ ^[0-9]+$ ]] && (( source_prefix >= 16 && source_prefix <= 32 )) || {
    echo "ERROR: source scopes broader than /16 are refused" >&2
    exit 2
}
source_network="${source_ip}/${source_prefix}"

runner=()
if [[ "${EUID}" -ne 0 ]]; then
    command -v sudo >/dev/null 2>&1 || {
        echo "ERROR: root or sudo is required to configure the sender firewall" >&2
        exit 1
    }
    sudo -n true >/dev/null 2>&1 || {
        echo "ERROR: non-interactive sudo access is required to configure the sender firewall" >&2
        exit 1
    }
    runner=(sudo -n)
fi

tcp_rule="rule family=ipv4 source address=${source_network} port port=6400-6600 protocol=tcp accept"
# Releases before the first-party direct-target sender also opened mDNS.
# Discovery is not used now, but remove cleans that obsolete rule if present.
legacy_mdns_rule="rule family=ipv4 source address=${source_network} port port=5353 protocol=udp accept"
nft_drop_in=/etc/nftables.d/omt-test-sender.nft
nft_rule="ip saddr ${source_network} tcp dport 6400-6600 accept"

query_rule() {
    local permanence="$1"
    local rule="$2"
    local -a permanent_arg=()
    [[ "${permanence}" == "permanent" ]] && permanent_arg=(--permanent)
    "${runner[@]}" firewall-cmd "${permanent_arg[@]}" --zone="${zone}" \
        --query-rich-rule="${rule}" >/dev/null
}

add_rule() {
    local permanence="$1"
    local rule="$2"
    local -a permanent_arg=()
    [[ "${permanence}" == "permanent" ]] && permanent_arg=(--permanent)
    query_rule "${permanence}" "${rule}" ||
        "${runner[@]}" firewall-cmd "${permanent_arg[@]}" --zone="${zone}" \
            --add-rich-rule="${rule}" >/dev/null
}

remove_rule() {
    local permanence="$1"
    local rule="$2"
    local -a permanent_arg=()
    [[ "${permanence}" == "permanent" ]] && permanent_arg=(--permanent)
    query_rule "${permanence}" "${rule}" &&
        "${runner[@]}" firewall-cmd "${permanent_arg[@]}" --zone="${zone}" \
            --remove-rich-rule="${rule}" >/dev/null || true
}

nft_reload() {
    if [[ -f /etc/nftables.nft ]]; then
        "${runner[@]}" nft -f /etc/nftables.nft
    fi
}

nft_rule_present() {
    "${runner[@]}" nft list chain inet filter input 2>/dev/null | \
        grep -F "${source_ip}" | grep -Fq "tcp dport 6400-6600"
}

use_firewalld=false
use_nft=false
if command -v firewall-cmd >/dev/null 2>&1; then
    use_firewalld=true
elif command -v nft >/dev/null 2>&1; then
    use_nft=true
else
    echo "ERROR: firewalld or nftables is required; see docs/OMT_TEST_SENDER.md" >&2
    exit 1
fi

case "${action}" in
    allow)
        if [[ "${use_firewalld}" == true ]]; then
            add_rule runtime "${tcp_rule}"
            add_rule permanent "${tcp_rule}"
            echo "Allowed OMT TCP 6400-6600 from ${source_network} in zone ${zone}"
        fi
        if [[ "${use_nft}" == true ]]; then
            "${runner[@]}" mkdir -p /etc/nftables.d
            nft_tmp="$(mktemp)"
            printf 'table inet filter {\n    chain input {\n        %s\n    }\n}\n' \
                "${nft_rule}" > "${nft_tmp}"
            "${runner[@]}" install -m 0600 "${nft_tmp}" "${nft_drop_in}"
            rm -f -- "${nft_tmp}"
            nft_reload
            if ! nft_rule_present; then
                "${runner[@]}" nft add rule inet filter input \
                    ip saddr "${source_network}" tcp dport 6400-6600 accept
            fi
            echo "Allowed OMT TCP 6400-6600 from ${source_network} in nftables"
        fi
        ;;
    remove)
        if [[ "${use_firewalld}" == true ]]; then
            remove_rule runtime "${tcp_rule}"
            remove_rule runtime "${legacy_mdns_rule}"
            remove_rule permanent "${tcp_rule}"
            remove_rule permanent "${legacy_mdns_rule}"
            echo "Removed OMT sender firewall rules for ${source_network} from zone ${zone}"
        fi
        if [[ "${use_nft}" == true ]]; then
            "${runner[@]}" rm -f -- "${nft_drop_in}"
            nft_reload
            echo "Removed OMT sender nftables rules for ${source_network}"
        fi
        ;;
    status)
        failed=0
        if [[ "${use_firewalld}" == true ]]; then
            for permanence in runtime permanent; do
                if query_rule "${permanence}" "${tcp_rule}"; then
                    echo "${permanence}: present: ${tcp_rule}"
                else
                    echo "${permanence}: absent: ${tcp_rule}"
                    failed=1
                fi
            done
        fi
        if [[ "${use_nft}" == true ]]; then
            if nft_rule_present; then
                echo "nftables: present: ${nft_rule}"
            else
                echo "nftables: absent: ${nft_rule}"
                failed=1
            fi
        fi
        exit "${failed}"
        ;;
    *)
        usage
        exit 2
        ;;
esac
