#!/bin/bash
# Idempotently allow or remove source-scoped OMT sender traffic in firewalld.
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

source_network="$(python3 - "${source_input}" <<'PY'
import ipaddress
import sys

try:
    network = ipaddress.ip_network(sys.argv[1], strict=False)
except ValueError as error:
    raise SystemExit(f"ERROR: invalid source IPv4 address or CIDR: {error}") from error
if network.version != 4:
    raise SystemExit("ERROR: the OMT test sender firewall supports IPv4 sources only")
if network.prefixlen < 16:
    raise SystemExit("ERROR: source scopes broader than /16 are refused")
print(network.with_prefixlen)
PY
)"

command -v firewall-cmd >/dev/null 2>&1 || {
    echo "ERROR: firewalld is required; see docs/OMT_TEST_SENDER.md for manual rules" >&2
    exit 1
}
runner=()
if [[ "${EUID}" -ne 0 ]]; then
    command -v sudo >/dev/null 2>&1 || {
        echo "ERROR: root or sudo is required to configure firewalld" >&2
        exit 1
    }
    sudo -n true >/dev/null 2>&1 || {
        echo "ERROR: non-interactive sudo access is required to configure firewalld" >&2
        exit 1
    }
    runner=(sudo -n)
fi

tcp_rule="rule family=ipv4 source address=${source_network} port port=6400-6600 protocol=tcp accept"
# Releases before the first-party direct-target sender also opened mDNS.
# Discovery is not used now, but remove cleans that obsolete rule if present.
legacy_mdns_rule="rule family=ipv4 source address=${source_network} port port=5353 protocol=udp accept"

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

case "${action}" in
    allow)
        add_rule runtime "${tcp_rule}"
        add_rule permanent "${tcp_rule}"
        echo "Allowed OMT TCP 6400-6600 from ${source_network} in zone ${zone}"
        ;;
    remove)
        remove_rule runtime "${tcp_rule}"
        remove_rule runtime "${legacy_mdns_rule}"
        remove_rule permanent "${tcp_rule}"
        remove_rule permanent "${legacy_mdns_rule}"
        echo "Removed OMT sender firewall rules for ${source_network} from zone ${zone}"
        ;;
    status)
        failed=0
        for permanence in runtime permanent; do
            if query_rule "${permanence}" "${tcp_rule}"; then
                echo "${permanence}: present: ${tcp_rule}"
            else
                echo "${permanence}: absent: ${tcp_rule}"
                failed=1
            fi
        done
        exit "${failed}"
        ;;
    *)
        usage
        exit 2
        ;;
esac
