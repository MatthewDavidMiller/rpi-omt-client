#!/bin/sh
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
#
# The deployer CLI's contract, which is the surface an operator and any script
# wrapping it actually touch. Exit 2 is a usage failure that leaves the command
# unrun; exit 1 is a command that ran and failed. Nothing here reaches the
# network: every invocation is either local-only or rejected before a connection
# is attempted, so this gate is deterministic on a workstation with no Pi.
set -eu

deployer="$1"
project="$2"
failures=0

fail() {
    echo "FAIL: $1" >&2
    failures=$((failures + 1))
}

# Assert an invocation's exit status without letting `set -e` abort on the
# non-zero ones being tested.
expect_status() {
    expected="$1"
    description="$2"
    shift 2
    actual=0
    # Closed stdin, always: a `--secrets-stdin` case that is expected to be
    # rejected before it reads would otherwise wait on the caller's terminal
    # forever, which is a hung gate rather than a failed one.
    "$@" >/dev/null 2>&1 </dev/null || actual=$?
    [ "${actual}" -eq "${expected}" ] ||
        fail "${description}: expected exit ${expected}, got ${actual}"
}

# As above, but the command reads its secrets from stdin.
expect_status_stdin() {
    expected="$1"
    description="$2"
    payload="$3"
    shift 3
    actual=0
    printf '%s' "${payload}" | "$@" >/dev/null 2>&1 || actual=$?
    [ "${actual}" -eq "${expected}" ] ||
        fail "${description}: expected exit ${expected}, got ${actual}"
}

version="$(${deployer} --version)"
[ -n "${version}" ] || fail "--version printed nothing"

# `check` is the only command that needs no remote, so it is where the capsule
# contract is exercised end to end. With no --project it validates the capsule
# compiled into the binary, which is what an operator's deployment uploads.
expect_status 0 "check validates the embedded capsule with no project at all" \
    "${deployer}" check
expect_status 0 "check accepts this project's capsule" \
    "${deployer}" --project "${project}" check

# The embedded report names the archive and its digest: a single-file deployer
# has nothing on disk for an operator to inspect instead.
embedded="$(${deployer} check)"
case "${embedded}" in
    *'omt-client-arm64.tar.gz'*'MiB'*'sha256 '*) ;;
    *) fail "check does not describe the embedded appliance image: ${embedded}" ;;
esac

empty="$(mktemp -d "${TMPDIR:-/tmp}/omt-deployer-cli.XXXXXX")"
trap 'rm -rf "${empty}"' EXIT INT TERM
expect_status 1 "check reports a directory with no manifest as a failed run" \
    "${deployer}" --project "${empty}" check

# `prerequisites` describes the workstation, so it needs no remote either. With
# no --project there is nothing for it to require: the deployment builds
# nothing and reads nothing from this machine.
expect_status 0 "prerequisites is satisfied by the embedded capsule alone" \
    "${deployer}" prerequisites
case "$(${deployer} prerequisites)" in
    *'[ok] Embedded appliance capsule'*) ;;
    *) fail "prerequisites does not report the embedded capsule" ;;
esac
case "$(${deployer} prerequisites)" in
    *'Container engine'*) fail "an embedded deployment asked for a container engine" ;;
    *) ;;
esac

# A project root is the developer override, and then the build tooling is
# reported again. A directory that is not a project is a failed run rather than
# a usage error: the command ran, and what it found is the failure.
expect_status 0 "prerequisites accepts this workstation" \
    "${deployer}" --project "${project}" prerequisites
expect_status 1 "prerequisites reports a directory with no capsule as a failed run" \
    "${deployer}" --project "${empty}" prerequisites

# Every row is a line, and the failure names what is missing rather than only
# that something is.
report="$(${deployer} --project "${empty}" prerequisites 2>&1 || true)"
case "${report}" in
    *'[MISSING] Project source tree'*) ;;
    *) fail "prerequisites does not name the missing entry: ${report}" ;;
esac
case "$(${deployer} --project "${project}" prerequisites)" in
    *'[ok] Container engine'*) ;;
    *) fail "prerequisites does not report the container engine" ;;
esac

# Missing required arguments leave the command unrun.
expect_status 2 "deploy without a host" "${deployer}" deploy
# Rebuilding is the one deployment that still needs a source tree, and clap
# refuses it before anything is opened rather than failing mid-deployment.
expect_status 2 "deploy --rebuild-image without a project" \
    "${deployer}" --host pi.local --username root deploy --rebuild-image
expect_status 2 "an unknown subcommand" "${deployer}" --project "${project}" frobnicate
expect_status 2 "an unknown option" "${deployer}" --project "${project}" --colour red check

# Connection arguments are validated before anything is opened.
expect_status 2 "a management action without a host" "${deployer}" --username root status
expect_status 2 "a management action without a username" "${deployer}" --host pi.local status
expect_status 2 "reboot without a host" "${deployer}" --username root reboot
expect_status 2 "a management action with no password" \
    "${deployer}" --host pi.local --username root status
expect_status 2 "an invalid host" \
    "${deployer}" --host '-pi.local' --username root --secrets-stdin status
expect_status 2 "an invalid username" \
    "${deployer}" --host pi.local --username 'ro ot' --secrets-stdin status
expect_status_stdin 2 "a missing explicit known_hosts file" '{"password":"secret"}' \
    "${deployer}" --host pi.local --username root \
    --known-hosts "${empty}/missing-known-hosts" --secrets-stdin status

# Secrets arrive on stdin or through a prompt, never as arguments.
expect_status 2 "two secret sources at once" \
    "${deployer}" --host pi.local --username root --secrets-stdin --interactive-secrets status
expect_status_stdin 2 "malformed secrets JSON" 'not json' \
    "${deployer}" --host pi.local --username root --secrets-stdin status
expect_status_stdin 2 "an unknown secrets field" '{"totp":"1"}' \
    "${deployer}" --host pi.local --username root --secrets-stdin status
oversized="$(awk 'BEGIN { printf "{\"password\":\""; for (i = 0; i < 17000; i++) printf "a"; printf "\"}" }')"
expect_status_stdin 2 "secrets JSON past the 16 KiB limit" "${oversized}" \
    "${deployer}" --host pi.local --username root --secrets-stdin status
expect_status_stdin 2 "a secret containing a control character" '{"password":"a\u0007b"}' \
    "${deployer}" --host pi.local --username root --secrets-stdin status

# wifi refuses to run rather than prompting when no secret source is offered.
expect_status 2 "wifi with no password source" \
    "${deployer}" --host pi.local --username root wifi --ssid studio
expect_status_stdin 2 "wifi with an invalid passphrase" \
    '{"password":"pw","wifi_password":"short"}' \
    "${deployer}" --host pi.local --username root --secrets-stdin wifi --ssid studio

# Web passwords use the same stdin-only secret channel and are validated before
# any SSH connection is opened.
expect_status 2 "web-password with no password source" \
    "${deployer}" --host pi.local --username root web-password
expect_status_stdin 2 "web-password rejects a short password" \
    '{"password":"pw","web_password":"too-short"}' \
    "${deployer}" --host pi.local --username root --secrets-stdin web-password

# Alpine sys setup is local-validated before any SSH session is opened. Its
# script is embedded too, so it needs no project root either.
expect_status 2 "alpine-setup with no secret source" \
    "${deployer}" --host pi.local --username root alpine-setup --hostname omt-client
expect_status_stdin 2 "alpine-setup without host passwords" '{"password":""}' \
    "${deployer}" --host pi.local --username root \
    --secrets-stdin alpine-setup --hostname omt-client
expect_status_stdin 2 "alpine-setup rejects a short root password" \
    '{"password":"","root_password":"short","pi_password":"longenough"}' \
    "${deployer}" --host pi.local --username root \
    --secrets-stdin alpine-setup --hostname omt-client
expect_status_stdin 2 "alpine-setup rejects an invalid hostname" \
    '{"password":"","root_password":"rootpass1","pi_password":"pipassword"}' \
    "${deployer}" --host pi.local --username root \
    --secrets-stdin alpine-setup --hostname '-bad'

# The JSON surface is one object per line, which is what a wrapper parses.
json="$(${deployer} --project "${project}" --json check)"
case "${json}" in
    '{"event":"result","message":"'*'","success":true}') ;;
    *) fail "check --json is not the documented line: ${json}" ;;
esac
[ "$(printf '%s\n' "${json}" | wc -l)" -eq 1 ] || fail "check --json emitted more than one line"

error_json="$(${deployer} --project "${empty}" --json check 2>/dev/null || true)"
case "${error_json}" in
    *'"event":"error"'*'"success":false'*) ;;
    *) fail "a failed run's --json output is not an error line: ${error_json}" ;;
esac

if [ "${failures}" -ne 0 ]; then
    echo "${failures} deployer CLI contract(s) failed" >&2
    exit 1
fi

echo "deployer CLI contracts passed"
