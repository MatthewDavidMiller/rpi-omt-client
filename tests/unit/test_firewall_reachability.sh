#!/bin/bash
# The appliance firewall must not lock the operator out of the appliance.
#
# This is a live packet test in a throwaway network namespace, not a grep over
# the ruleset text. The bug it exists for looked completely correct on paper:
# the drop-in accepted SSH and the web port, but it did so in its own table
# hooked at priority -20, and netfilter runs every base chain on a hook. An
# `accept` only ends the chain it is in, so Alpine's stock input chain at
# priority 0 dropped the packet afterwards and the Pi went dark on every port.
#
# Only a real connection attempt distinguishes those two rulesets, so that is
# what this does.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
INSTALL="${ROOT}/deploy/host/install.sh"
NS=omt-fw-test
HOST_IP=10.99.213.1
NS_IP=10.99.213.2
SSH_PORT=22
WEB_PORT=5000

for tool in ip nft python3; do
    command -v "${tool}" >/dev/null 2>&1 || {
        echo "FAIL: ${tool} is required for the firewall reachability gate" >&2
        exit 1
    }
done
# Needs real network administration; there is no meaningful unprivileged form.
if [[ "$(id -u)" -ne 0 ]]; then
    if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
        exec sudo -n env NFT_TEST_REENTRY=1 bash "${BASH_SOURCE[0]}" "$@"
    fi
    echo "FAIL: this gate needs root (passwordless sudo) to create a netns" >&2
    exit 1
fi

cleanup() {
    ip netns del "${NS}" >/dev/null 2>&1 || true
    ip link del omt-fw-h >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

ip netns add "${NS}"
ip link add omt-fw-h type veth peer name omt-fw-n
ip link set omt-fw-n netns "${NS}"
ip addr add "${HOST_IP}/24" dev omt-fw-h
ip link set omt-fw-h up
ip netns exec "${NS}" ip addr add "${NS_IP}/24" dev omt-fw-n
ip netns exec "${NS}" ip link set omt-fw-n up
ip netns exec "${NS}" ip link set lo up

# Listeners on the two ports the appliance must keep reachable.
for port in "${SSH_PORT}" "${WEB_PORT}"; do
    ip netns exec "${NS}" timeout 60 python3 -c "
import socket, sys
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('${NS_IP}', ${port}))
s.listen(8)
while True:
    try:
        c, _ = s.accept()
        c.close()
    except Exception:
        break
" >/dev/null 2>&1 &
done
sleep 2

# Alpine's stock /etc/nftables.nft: this is the ruleset the appliance lands on
# top of, and the reason a private table is not enough.
ip netns exec "${NS}" nft -f - <<'STOCK'
table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;
        iifname lo accept
        ct state { established, related } accept
        ct state invalid drop
    }
}
STOCK

# Extract the drop-in exactly as the installer writes it, so this test cannot
# drift away from the shipped ruleset.
DROP_IN="$(sed -n '/^host_publish_file \/etc\/nftables.d\/omt-client.nft/,/^EOF$/p' "${INSTALL}" \
    | sed '1d;$d' \
    | sed "s/\${SSH_PORT}/${SSH_PORT}/g; s/\${WEB_PORT}/${WEB_PORT}/g")"
[[ -n "${DROP_IN}" ]] || {
    echo "FAIL: could not extract the nftables drop-in from install.sh" >&2
    exit 1
}
printf '%s\n' "${DROP_IN}" | ip netns exec "${NS}" nft -f - || {
    echo "FAIL: the installer's nftables drop-in is not loadable" >&2
    exit 1
}

failures=0
probe() {
    local port="$1" expect="$2" label="$3" result
    if timeout 4 bash -c "cat < /dev/null > /dev/tcp/${NS_IP}/${port}" 2>/dev/null; then
        result=reachable
    else
        result=blocked
    fi
    if [[ "${result}" != "${expect}" ]]; then
        echo "FAIL: ${label} on port ${port} was ${result}, expected ${expect}" >&2
        failures=$((failures + 1))
    fi
}

probe "${SSH_PORT}" reachable "SSH must survive the appliance firewall"
probe "${WEB_PORT}" reachable "the web UI must survive the appliance firewall"
# A port the appliance never opens must still be refused, or the drop-in has
# simply disabled the firewall rather than punched two holes in it.
probe 4444 blocked "an unrelated port must stay closed"

if ((failures > 0)); then
    echo "${failures} firewall reachability test(s) failed" >&2
    exit 1
fi

echo "Firewall reachability tests passed"
