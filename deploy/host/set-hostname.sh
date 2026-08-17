#!/bin/sh
# Change an installed appliance's hostname.
#
# Usage: printf '%s\n' NEWNAME | sudo /bin/sh set-hostname.sh
#
# The name arrives on stdin rather than in argv for the same reason the Alpine
# sys installer takes its values there: nothing this script is given is ever
# interpolated into another command line, so a process listing on the Pi shows
# the script and nothing else.
#
# This is deliberately standalone. It is uploaded to /tmp and run against an
# appliance that is already installed, so it cannot source deploy/lib -- an
# operator renaming a Pi must not first have to redeploy the whole capsule.
set -eu
export LC_ALL=C
umask 022

COMPLETE_MARKER="=== Appliance hostname set ==="
SCRATCH=""
cleanup() {
    [ -z "${SCRATCH}" ] || rm -f -- "${SCRATCH}"
}
trap cleanup EXIT INT TERM

[ "$(id -u)" = 0 ] || {
    echo "ERROR: run this as root." >&2
    exit 1
}

IFS= read -r NEW_HOSTNAME || {
    echo "ERROR: hostname was not provided on stdin." >&2
    exit 1
}

# The same single DNS label setup-sys.sh accepts. Keep the two in step: an
# appliance renamed here has to stay a name the factory installer would have
# been willing to write in the first place.
case "${NEW_HOSTNAME}" in
    [A-Za-z0-9]|[A-Za-z0-9][-A-Za-z0-9]*[A-Za-z0-9])
        [ "${#NEW_HOSTNAME}" -le 63 ] || {
            echo "ERROR: hostname must be at most 63 characters." >&2
            exit 1
        }
        ;;
    *)
        echo "ERROR: hostname must be a single DNS label." >&2
        exit 1
        ;;
esac

OLD_HOSTNAME="$(hostname 2>/dev/null || echo unknown)"

# Replace $1 with this command's stdin, atomically, at mode $2.
#
# Never called through a pipe: a `sh` without `pipefail` runs the right-hand
# side of one in a subshell, where the refusals below would exit that subshell
# and let the script continue as though the file had been written.
publish() {
    publish_path="$1"
    publish_mode="$2"
    if [ -e "${publish_path}" ] || [ -L "${publish_path}" ]; then
        [ -f "${publish_path}" ] && [ ! -L "${publish_path}" ] || {
            echo "ERROR: ${publish_path} is not a regular file." >&2
            exit 1
        }
    fi
    publish_tmp="$(mktemp "${publish_path}.omt-hostname.XXXXXX")"
    cat > "${publish_tmp}"
    chmod "${publish_mode}" "${publish_tmp}"
    chown root:root "${publish_tmp}"
    mv -f -- "${publish_tmp}" "${publish_path}"
}

SCRATCH="$(mktemp /tmp/omt-hostname.XXXXXX)"

echo "Setting hostname to ${NEW_HOSTNAME} (was ${OLD_HOSTNAME})..."
printf '%s\n' "${NEW_HOSTNAME}" > "${SCRATCH}"
publish /etc/hostname 0644 < "${SCRATCH}"
hostname "${NEW_HOSTNAME}"

# A loopback entry for the machine's own name, replacing whatever this wrote
# before. Without one the box cannot resolve itself, which surfaces as a
# multi-second stall in sudo and in Avahi's startup rather than as a name
# error. 127.0.1.1 rather than 127.0.0.1, so the localhost line keeps its
# canonical name.
if [ -f /etc/hosts ] && [ ! -L /etc/hosts ]; then
    awk -v name="${NEW_HOSTNAME}" '
        $1 == "127.0.1.1" { next }
        { print }
        END { printf "127.0.1.1\t%s\n", name }
    ' /etc/hosts > "${SCRATCH}"
    publish /etc/hosts 0644 < "${SCRATCH}"
fi

# The DHCP client identity. setup-sys.sh writes one `hostname` option per
# configured interface; this rewrites those in place and leaves every other
# line of the operator's network configuration exactly as it was. It takes
# effect at the next lease, which is why nothing here renews one: this script
# very often runs over the SSH session that lease is carrying.
if [ -f /etc/network/interfaces ] && [ ! -L /etc/network/interfaces ]; then
    awk -v name="${NEW_HOSTNAME}" '
        $1 == "hostname" && NF == 2 {
            match($0, /^[ \t]*/)
            printf "%shostname %s\n", substr($0, 1, RLENGTH), name
            next
        }
        { print }
    ' /etc/network/interfaces > "${SCRATCH}"
    publish /etc/network/interfaces 0644 < "${SCRATCH}"
fi

# Avahi advertises the system hostname as <name>.local and reads it once, at
# startup. Restarting it is what makes the new name resolvable on the network
# now rather than at the next boot.
if [ -x /etc/init.d/avahi-daemon ]; then
    echo "Restarting Avahi so ${NEW_HOSTNAME}.local resolves..."
    rc-service avahi-daemon restart >/dev/null 2>&1 ||
        echo "WARNING: Avahi did not restart; ${NEW_HOSTNAME}.local updates at the next boot." >&2
fi

# The Web GUI's name comes from the container's /etc/hostname, and Docker
# writes that file once, when the container is created: a host-network
# container inherits the host's name at that moment and never looks again. So
# restarting the process inside the container would change nothing, and only a
# new container carries the new name. The OpenRC service's stop is
# `compose down`, which removes the container, so its restart is exactly that
# recreation.
#
# A stopped appliance is left stopped. A first install defers startup until the
# reboot that loads the KMS settings, and starting it early here would fail on
# the DRM device that reboot is what provides.
if [ -x /etc/init.d/omt-client ] && rc-service omt-client status >/dev/null 2>&1; then
    echo "Recreating the appliance container so the Web GUI shows ${NEW_HOSTNAME}..."
    rc-service omt-client restart || {
        echo "ERROR: the hostname is set, but the appliance did not come back." >&2
        echo "Check 'sudo rc-service omt-client status' on the Pi." >&2
        exit 1
    }
else
    echo "The appliance is not running; it will use ${NEW_HOSTNAME} when it starts."
fi

echo
echo "${COMPLETE_MARKER}"
echo "Hostname:  ${NEW_HOSTNAME} (was ${OLD_HOSTNAME})"
echo "mDNS:      ${NEW_HOSTNAME}.local"
echo "DHCP/DNS:  the new name is registered at the next lease renewal or reboot."
echo "SSH:       this session is unaffected; reconnect by IP or by the new name."
