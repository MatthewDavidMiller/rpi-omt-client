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

# Reputable US HTTPS Alpine mirrors. Keep this list identical in
# setup-sys.sh and install.sh; tests/unit/test_setup_sys.sh compares them.
# BEGIN US HTTPS APK MIRRORS
US_HTTPS_APK_MIRRORS="
https://mirrors.edge.kernel.org/alpine
https://mirrors.ocf.berkeley.edu/alpine
https://mirror.math.princeton.edu/pub/alpinelinux
"
# END US HTTPS APK MIRRORS

if ! [ -f /etc/ssl/certs/ca-certificates.crt ]; then
    echo "Installing CA certificates so apk can use HTTPS mirrors..."
    apk add --no-cache ca-certificates || true
fi
APK_MIRROR_TMP="$(mktemp)"
APK_MIRROR_OK=
for APK_MIRROR_BASE in ${US_HTTPS_APK_MIRRORS}; do
    echo "Trying US HTTPS apk mirror ${APK_MIRROR_BASE}..."
    printf '%s/v%s/main\n' "${APK_MIRROR_BASE}" "${SUPPORTED_ALPINE_SERIES}" > "${APK_MIRROR_TMP}"
    printf '%s/v%s/community\n' "${APK_MIRROR_BASE}" "${SUPPORTED_ALPINE_SERIES}" >> "${APK_MIRROR_TMP}"
    cp "${APK_MIRROR_TMP}" /etc/apk/repositories
    if apk update; then
        echo "Pinned apk repositories to ${APK_MIRROR_BASE} (HTTPS)."
        APK_MIRROR_OK=yes
        break
    fi
    echo "Mirror ${APK_MIRROR_BASE} did not serve an index; trying the next US HTTPS mirror."
done
rm -f -- "${APK_MIRROR_TMP}"
[ "${APK_MIRROR_OK}" = yes ] || {
    echo "ERROR: no reputable US HTTPS apk mirror responded." >&2
    exit 1
}

echo "Installing the bootstrap prerequisites (bash, sudo)..."
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
