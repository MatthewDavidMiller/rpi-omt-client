#!/bin/bash
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
#
# The first-party SSH client against a real OpenSSH server.
#
# An unprivileged sshd on loopback, with throwaway host and user keys made by
# ssh-keygen, is restricted to one algorithm at a time so every key exchange,
# cipher, MAC, and host-key algorithm the client offers is proven to
# interoperate. Then the user-key formats (including passphrase-protected
# OpenSSH keys, which exercise bcrypt_pbkdf), both upload paths, the PTY
# marker gating Alpine's su needs, and every host-key refusal.
#
# Usage: tests/integration/test_ssh_client.sh <ssh_interop> <rpi-omt-deploy>
set -euo pipefail

INTEROP="$(realpath "$1")"
DEPLOYER="$(realpath "$2")"
SSHD="$(command -v sshd || echo /usr/sbin/sshd)"
SFTP_SERVER=""
for candidate in /usr/lib/ssh/sftp-server /usr/lib/openssh/sftp-server /usr/libexec/sftp-server; do
    [[ -x "${candidate}" ]] && SFTP_SERVER="${candidate}" && break
done
[[ -x "${SSHD}" && -n "${SFTP_SERVER}" ]] || {
    echo "ERROR: sshd and sftp-server are required (the toolbox provides them)" >&2
    exit 1
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/omt-ssh-client.XXXXXX")"
SSHD_PID=""
cleanup() {
    [[ -n "${SSHD_PID}" ]] && kill "${SSHD_PID}" 2>/dev/null && wait "${SSHD_PID}" 2>/dev/null
    rm -rf "${WORK}"
}
trap cleanup EXIT INT TERM
cd "${WORK}"

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
USER_NAME="$(id -un)"
failures=0
pass() { printf 'PASS: %s\n' "$1"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; failures=$((failures + 1)); }

ssh-keygen -q -t ed25519 -f host_ed25519 -N ''
ssh-keygen -q -t ecdsa -b 384 -f host_ecdsa -N ''
ssh-keygen -q -t rsa -b 3072 -f host_rsa -N ''
ssh-keygen -q -t ed25519 -f user_ed25519 -N ''
ssh-keygen -q -t ed25519 -f user_ed25519_enc -N 'correct horse'
ssh-keygen -q -t ecdsa -b 256 -f user_ecdsa_enc -N 'correct horse'
ssh-keygen -q -t rsa -b 3072 -f user_rsa -N ''
ssh-keygen -q -t ecdsa -b 521 -f user_ecdsa_pem -N '' -m PEM
ssh-keygen -q -t ed25519 -f stranger -N ''
cat user_ed25519.pub user_ed25519_enc.pub user_ecdsa_enc.pub user_rsa.pub user_ecdsa_pem.pub >authorized_keys
for key in host_ed25519 host_ecdsa host_rsa; do
    printf '[127.0.0.1]:%s %s\n' "${PORT}" "$(cut -d' ' -f1,2 "${key}.pub")"
done >known_hosts

cat >sshd_config <<EOF
Port ${PORT}
ListenAddress 127.0.0.1
HostKey ${WORK}/host_ed25519
HostKey ${WORK}/host_ecdsa
HostKey ${WORK}/host_rsa
PidFile ${WORK}/sshd.pid
AuthorizedKeysFile ${WORK}/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
Subsystem sftp ${SFTP_SERVER}
EOF

start_sshd() {
    "${SSHD}" -D -e -f "${WORK}/sshd_config" "$@" >"${WORK}/sshd.log" 2>&1 &
    SSHD_PID=$!
    for _ in $(seq 50); do
        (exec 3<>"/dev/tcp/127.0.0.1/${PORT}") 2>/dev/null && return 0
        sleep 0.1
    done
    echo "ERROR: sshd did not start:" >&2
    cat "${WORK}/sshd.log" >&2
    exit 1
}
stop_sshd() {
    kill "${SSHD_PID}" 2>/dev/null || true
    wait "${SSHD_PID}" 2>/dev/null || true
    SSHD_PID=""
}

# interop <known_hosts> <key|-> <mode> <args...>; output in ${WORK}/out
interop() {
    local known="$1" key="$2"
    shift 2
    timeout 60 "${INTEROP}" 127.0.0.1 "${PORT}" "${USER_NAME}" "${known}" "${key}" "$@" \
        >"${WORK}/out" 2>&1
}

expect_ok() {
    local description="$1" expected="$2"
    shift 2
    if interop "$@" && grep -Fq -- "${expected}" "${WORK}/out"; then
        pass "${description}"
    else
        fail "${description}: $(head -c 400 "${WORK}/out")"
    fi
}

expect_refused() {
    local description="$1" expected="$2"
    shift 2
    if interop "$@"; then
        fail "${description}: the connection was accepted"
    elif grep -Fq -- "${expected}" "${WORK}/out" && ! grep -q 'Sanitizer' "${WORK}/out"; then
        pass "${description}"
    else
        fail "${description}: $(head -c 400 "${WORK}/out")"
    fi
}

KH="${WORK}/known_hosts"
KEY="${WORK}/user_ed25519"

# One algorithm at a time on the server side.
for option in \
    KexAlgorithms=mlkem768x25519-sha256 KexAlgorithms=curve25519-sha256 \
    KexAlgorithms=curve25519-sha256@libssh.org \
    KexAlgorithms=diffie-hellman-group-exchange-sha256 \
    KexAlgorithms=diffie-hellman-group18-sha512 KexAlgorithms=diffie-hellman-group16-sha512 \
    KexAlgorithms=diffie-hellman-group14-sha256 \
    Ciphers=chacha20-poly1305@openssh.com Ciphers=aes256-gcm@openssh.com \
    Ciphers=aes128-gcm@openssh.com Ciphers=aes256-ctr Ciphers=aes192-ctr Ciphers=aes128-ctr \
    HostKeyAlgorithms=ssh-ed25519 HostKeyAlgorithms=ecdsa-sha2-nistp384 \
    HostKeyAlgorithms=rsa-sha2-512 HostKeyAlgorithms=rsa-sha2-256; do
    start_sshd -o "${option}"
    expect_ok "${option}" "exit=0" "${KH}" "${KEY}" run 'echo negotiated'
    stop_sshd
done
for mac in hmac-sha2-512-etm@openssh.com hmac-sha2-256-etm@openssh.com hmac-sha2-512 hmac-sha2-256; do
    start_sshd -o Ciphers=aes128-ctr -o "MACs=${mac}"
    expect_ok "MACs=${mac}" "exit=0" "${KH}" "${KEY}" run 'echo negotiated'
    stop_sshd
done

start_sshd
expect_ok "stdout, stderr, and the exit status are kept apart" "exit=3" \
    "${KH}" "${KEY}" run 'echo out; echo err >&2; exit 3'
grep -Fq 'stderr=err' "${WORK}/out" && pass "stderr is captured" || fail "stderr is captured"
expect_ok "stdin is delivered, then EOF" "line-one" "${KH}" "${KEY}" run 'cat' 'line-one'
expect_ok "a PTY command receives input only after its marker" "got=secret-value" \
    "${KH}" "${KEY}" pty 'stty -echo; printf "READY\n"; read v; echo "got=$v"' READY $'secret-value\n'
OMT_TEST_SECRET='correct horse' expect_ok "a passphrase-protected Ed25519 key" "exit=0" \
    "${KH}" "${WORK}/user_ed25519_enc" run true
OMT_TEST_SECRET='correct horse' expect_ok "a passphrase-protected ECDSA key" "exit=0" \
    "${KH}" "${WORK}/user_ecdsa_enc" run true
expect_ok "an RSA key signs with SHA-2" "exit=0" "${KH}" "${WORK}/user_rsa" run true
expect_ok "a PEM ECDSA P-521 key" "exit=0" "${KH}" "${WORK}/user_ecdsa_pem" run true
OMT_TEST_SECRET='wrong' expect_refused "a wrong passphrase" "incorrect passphrase" \
    "${KH}" "${WORK}/user_ed25519_enc" run true
expect_refused "an encrypted key without a passphrase" "no passphrase was given" \
    "${KH}" "${WORK}/user_ed25519_enc" run true
expect_refused "an unauthorized key" "SSH authentication failed" "${KH}" "${WORK}/stranger" run true

head -c 5000000 /dev/urandom >blob.bin
if interop "${KH}" "${KEY}" upload "${WORK}/blob.bin" "${WORK}/sftp.bin" && cmp -s blob.bin sftp.bin; then
    pass "an SFTP upload arrives intact"
else
    fail "an SFTP upload arrives intact: $(head -c 400 "${WORK}/out")"
fi
if interop "${KH}" "${KEY}" shell-upload "${WORK}/blob.bin" "${WORK}/shell.bin" &&
    cmp -s blob.bin shell.bin && [[ "$(stat -c %a shell.bin)" == 600 ]]; then
    pass "the cat fallback arrives intact and private"
else
    fail "the cat fallback arrives intact and private: $(head -c 400 "${WORK}/out")"
fi
expect_refused "an upload nobody can write names both failures" "shell fallback failed" \
    "${KH}" "${KEY}" upload-bytes 10 "${WORK}/missing/x.bin"

# Host keys: only a known, unrevoked key is accepted.
grep -v ed25519 "${KH}" >kh_ecdsa_only
expect_ok "negotiation prefers a key type known_hosts already trusts" "exit=0" \
    "${WORK}/kh_ecdsa_only" "${KEY}" run true
printf '[127.0.0.1]:%s %s\n' "${PORT}" "$(cut -d' ' -f1,2 user_ed25519.pub)" >kh_changed
expect_refused "a changed host key" "does not match" "${WORK}/kh_changed" "${KEY}" run true
printf 'elsewhere %s\n' "$(cut -d' ' -f1,2 host_ed25519.pub)" >kh_unknown
expect_refused "an unknown host" "is not in" "${WORK}/kh_unknown" "${KEY}" run true
{ cat "${KH}"; printf '@revoked * %s\n' "$(cut -d' ' -f1,2 host_ed25519.pub)"; } >kh_revoked
expect_refused "a revoked host key" "is revoked" "${WORK}/kh_revoked" "${KEY}" run true
expect_refused "a missing known_hosts" "known_hosts was not found" "${WORK}/nope" "${KEY}" run true
cp "${KH}" kh_hashed && ssh-keygen -H -f kh_hashed >/dev/null 2>&1
expect_ok "a hashed known_hosts" "exit=0" "${WORK}/kh_hashed" "${KEY}" run true

# The deployer end to end: an operation reaches the server and reports the
# remote failure it gets back.
out="$("${DEPLOYER}" --host 127.0.0.1 --port "${PORT}" --username "${USER_NAME}" \
    --key "${KEY}" --known-hosts "${KH}" --json deploy 2>&1 || true)"
if grep -Fq '"event":"error","message":"Remote platform probe failed' <<<"${out}"; then
    pass "a deployment refuses a host that is not an Alpine Raspberry Pi"
else
    fail "a deployment refuses a host that is not an Alpine Raspberry Pi: ${out}"
fi
stop_sshd

if ((failures > 0)); then
    echo "${failures} SSH client check(s) failed" >&2
    exit 1
fi
echo "SSH client interoperates with OpenSSH"
