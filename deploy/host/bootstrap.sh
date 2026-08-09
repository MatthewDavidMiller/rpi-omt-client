#!/bin/sh
# shellcheck shell=sh
# Prepare a stock Alpine Raspberry Pi for the OMT Client installer.
#
# This is the one script in the deployment set that may not assume bash: a
# clean Alpine image ships busybox ash and nothing else, so `install.sh`
# (#!/bin/bash) and `transaction.sh` cannot run until this has. It also
# installs sudo, which Alpine omits entirely -- the wheel group exists but has
# no way to escalate, so every `sudo` in the deploy path would fail.
#
# Run as root. It is idempotent and safe to re-run before every install.

set -eu
export LC_ALL=C
umask 022

SUPPORTED_ALPINE_SERIES=3.24

[ "$(id -u)" = 0 ] || {
    echo "ERROR: bootstrap.sh must run as root." >&2
    echo "On a stock Alpine host: su -c '/path/to/deploy/host/bootstrap.sh'" >&2
    exit 1
}
[ "$(uname -m)" = aarch64 ] || {
    echo "ERROR: This appliance supports only Alpine Linux aarch64." >&2
    exit 1
}
[ -r /etc/alpine-release ] || {
    echo "ERROR: This appliance supports only Alpine Linux." >&2
    exit 1
}
ALPINE_RELEASE="$(cat /etc/alpine-release)"
case "${ALPINE_RELEASE}" in
    "${SUPPORTED_ALPINE_SERIES}".*) ;;
    *)
        echo "ERROR: Alpine Linux ${SUPPORTED_ALPINE_SERIES}.x is required; detected ${ALPINE_RELEASE}." >&2
        exit 1
        ;;
esac

# Alpine keeps sudo in community, so the repository has to be live before the
# package list is resolvable. The series is read from the running system rather
# than hardcoded, so this keeps working across a supported series bump.
if ! grep -Eq '^[^#[:space:]]+/community/?$' /etc/apk/repositories; then
    MAIN_REPOSITORY="$(sed -n 's|^\([^#[:space:]]*\)/main/*$|\1|p' /etc/apk/repositories | head -n 1)"
    case "${MAIN_REPOSITORY}" in
        http://*|https://*) ;;
        *)
            echo "ERROR: Enable a trusted Alpine ${SUPPORTED_ALPINE_SERIES} main repository first." >&2
            exit 1
            ;;
    esac
    echo "Enabling the Alpine community repository..."
    printf '%s/community\n' "${MAIN_REPOSITORY}" >> /etc/apk/repositories
fi

echo "Installing the bootstrap prerequisites (bash, sudo)..."
apk update
apk add --no-cache bash sudo

# The installer and the deploy transaction are bash scripts; refuse to hand
# back control claiming success if either interpreter is still absent.
for required in bash sudo; do
    command -v "${required}" >/dev/null 2>&1 || {
        echo "ERROR: ${required} is still missing after package installation." >&2
        exit 1
    }
done

# Alpine creates the wheel group but grants it nothing. Both escalation paths
# are configured so an operator can use whichever their tooling expects.
install -d -m 0750 /etc/sudoers.d
SUDOERS_TMP="$(mktemp /etc/sudoers.d/.omt-client.XXXXXX)"
printf '%%wheel ALL=(ALL:ALL) ALL\n' > "${SUDOERS_TMP}"
chown root:root "${SUDOERS_TMP}"
chmod 0440 "${SUDOERS_TMP}"
# A malformed drop-in breaks sudo for every user, so validate before publishing.
visudo -cqf "${SUDOERS_TMP}" || {
    echo "ERROR: generated sudoers drop-in failed validation." >&2
    rm -f -- "${SUDOERS_TMP}"
    exit 1
}
mv -f "${SUDOERS_TMP}" /etc/sudoers.d/omt-client

# Alpine's doas package always ships /etc/doas.conf -- with every rule in it
# commented out -- so the previous guard, which wrote a config only when that
# file was absent, never fired once, and doas was left inert on exactly the
# stock images this script exists for.
# Publish a drop-in instead: /etc/doas.d/*.conf is read by the binary, and it
# leaves the packaged file, and any rule an operator has put in it, untouched.
# The temporary name deliberately does not end in .conf so a half-written file
# is never inside that glob.
if command -v doas >/dev/null 2>&1; then
    install -d -m 0755 /etc/doas.d
    DOAS_TMP="$(mktemp /etc/doas.d/.omt-client.XXXXXX)"
    printf 'permit persist :wheel\n' > "${DOAS_TMP}"
    chown root:root "${DOAS_TMP}"
    chmod 0640 "${DOAS_TMP}"
    mv -f "${DOAS_TMP}" /etc/doas.d/10-omt-client-wheel.conf
fi

echo "Bootstrap complete: bash and sudo are installed and wheel may escalate."
