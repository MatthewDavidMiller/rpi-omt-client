#!/bin/bash
# Run the amd64 appliance container and exercise HTTPS/auth, About, and reboot.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${PROJECT_ROOT}/scripts/docker-test-env.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
IMAGE_TAG="omt-client:smoke-test"
CONTAINER_NAME="omt-client-smoke"
ACK_CONTAINER_NAME="omt-client-reboot-ack"
PORT=15000
BASE_URL="https://127.0.0.1:${PORT}"
TEST_PASSWORD="smoke-test-password"
TEST_ROOT="$(mktemp -d)"
CONFIG_DIR="${TEST_ROOT}/config"
HOST_ACTIONS_DIR="${TEST_ROOT}/host-actions"
COOKIE_JAR="${TEST_ROOT}/cookies"

cleanup() {
    if [[ -n "${CONTAINER_ENGINE:-}" ]]; then
        "${CONTAINER_ENGINE}" exec "${CONTAINER_NAME}" \
            chmod -R a+rwX /etc/omt /host-actions >/dev/null 2>&1 || true
        "${CONTAINER_ENGINE}" rm -f \
            "${ACK_CONTAINER_NAME}" "${CONTAINER_NAME}" >/dev/null 2>&1 || true
        "${CONTAINER_ENGINE}" rmi "${IMAGE_TAG}" >/dev/null 2>&1 || true
    fi
    rm -rf "${TEST_ROOT}"
}
trap cleanup EXIT

pass() { printf "${GREEN}PASS${NC}: %s\n" "$1"; }
fail() {
    printf "${RED}FAIL${NC}: %s\n" "$1" >&2
    if [[ -n "${CONTAINER_ENGINE:-}" ]]; then
        "${CONTAINER_ENGINE}" logs "${CONTAINER_NAME}" 2>/dev/null || true
    fi
    exit 1
}
csrf_from() {
    grep -oE 'name="csrf_token" value="[^"]+"' |
        sed -E 's/.*value="([^"]+)"/\1/' |
        head -1
}
curl_app() {
    curl --silent --show-error --insecure \
        --connect-timeout 2 --max-time 15 \
        -H "Referer: ${BASE_URL}/" "$@"
}

echo "Native OMT Container Smoke Test"
echo "==============================="

# shellcheck disable=SC2310
ensure_test_container_engine || fail "Docker or Podman is required"
command -v curl >/dev/null 2>&1 || fail "curl is required"

mkdir -p "${CONFIG_DIR}" "${HOST_ACTIONS_DIR}"
chmod 777 "${CONFIG_DIR}" "${HOST_ACTIONS_DIR}"

cd "${PROJECT_ROOT}"
# shellcheck disable=SC2310
container_engine_build \
    -f deploy/Dockerfile \
    --build-arg RPI_OMT_CLIENT_VERSION=vtest \
    -t "${IMAGE_TAG}" . || fail "container image build failed"
pass "amd64 appliance image built"

config_volume="$(container_engine_volume "${CONFIG_DIR}" /etc/omt)"
if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
    actions_volume="${HOST_ACTIONS_DIR}:/host-actions:z"
else
    actions_volume="$(container_engine_volume "${HOST_ACTIONS_DIR}" /host-actions)"
fi

# The production installer creates these inodes as the image UID with mode
# 0600. Create them through the image so rootless and rootful engines see the
# same ownership.
# shellcheck disable=SC2016
"${CONTAINER_ENGINE}" run --rm \
    -e TEST_WEB_PASSWORD="${TEST_PASSWORD}" \
    -v "${config_volume}" \
    -v "${actions_volume}" \
    --entrypoint /bin/sh "${IMAGE_TAG}" \
    -c 'umask 077
        printf "%s\n" "${TEST_WEB_PASSWORD}" > /etc/omt/web_password
        : > /host-actions/reboot.request' ||
    fail "non-root config and reboot channel initialization failed"

# The tmpfs at /run/omt mirrors deploy/compose.yml: per-boot receiver state is
# kept off the SD-card-backed config volume. Running without it would only prove
# the image's own fallback works, and this is the configuration that ships.
"${CONTAINER_ENGINE}" run -d \
    --name "${CONTAINER_NAME}" \
    -p "${PORT}:5000" \
    -e OMT_REBOOT_ACK_TIMEOUT_SECONDS=5 \
    --tmpfs /run/omt:size=1m,mode=1777 \
    -v "${config_volume}" \
    -v "${actions_volume}" \
    "${IMAGE_TAG}" >/dev/null || fail "container failed to start"

for attempt in $(seq 1 30); do
    # shellcheck disable=SC2310
    if curl_app "${BASE_URL}/login" >/dev/null 2>&1; then
        pass "HTTPS Web GUI became ready"
        break
    fi
    if [[ "${attempt}" -eq 30 ]]; then
        fail "Web GUI did not become ready"
    fi
    sleep 1
done

# Assert the declared probe rather than the engine's scheduler: Podman runs image
# health checks through transient systemd units that are not available
# everywhere, so scheduling is not a portable contract but the command is.
# Docker exposes the image probe as .Config.Healthcheck, Podman as .HealthCheck,
# so match the whole inspect document rather than either engine's field path.
health_test="$("${CONTAINER_ENGINE}" inspect --format '{{json .}}' "${IMAGE_TAG}" 2>/dev/null)"
[[ "${health_test}" == *'"/opt/venv/bin/python"'* && "${health_test}" == *'/login'* ]] ||
    fail "image declares no usable HEALTHCHECK command"
pass "image declares a HEALTHCHECK command"

# Per-boot state must land on the tmpfs, in a directory the image user owns
# privately, and must leave nothing behind on the config volume. A regression
# here is silent: playback keeps working while the SD card takes the writes.
runtime_state="$("${CONTAINER_ENGINE}" exec "${CONTAINER_NAME}" /bin/sh -c '
    printf "%s %s %s\n" \
        "$(stat -c "%a:%U" "${OMT_RUNTIME_DIR}")" \
        "$(stat -f -c %T "${OMT_RUNTIME_DIR}")" \
        "$([ -e /etc/omt/run ] && echo legacy-present || echo legacy-absent)"')"
[[ "${runtime_state}" == "700:omt tmpfs legacy-absent" ]] ||
    fail "per-boot state is not private on tmpfs: ${runtime_state}"
pass "per-boot receiver state is a private tmpfs directory off the config volume"

"${CONTAINER_ENGINE}" exec "${CONTAINER_NAME}" \
    /opt/venv/bin/python -c \
    'import os, ssl, urllib.request; urllib.request.urlopen("https://127.0.0.1:" + os.environ["WEB_PORT"] + "/login", timeout=4, context=ssl._create_unverified_context())' ||
    fail "HEALTHCHECK probe failed against the live listener"
pass "HEALTHCHECK probe succeeds against the live listener"

unauthenticated_code="$(
    curl_app --output /dev/null --write-out '%{http_code}' "${BASE_URL}/about"
)"
[[ "${unauthenticated_code}" == "302" ]] ||
    fail "About page is not authentication-protected"
pass "About page redirects unauthenticated users"

login_page="$(curl_app --cookie-jar "${COOKIE_JAR}" "${BASE_URL}/login")"
login_csrf="$(printf '%s' "${login_page}" | csrf_from)"
[[ -n "${login_csrf}" ]] || fail "login CSRF token is missing"
login_code="$(
    curl_app --output /dev/null --write-out '%{http_code}' \
        --cookie "${COOKIE_JAR}" --cookie-jar "${COOKIE_JAR}" \
        --request POST "${BASE_URL}/login" \
        --data-urlencode "password=${TEST_PASSWORD}" \
        --data-urlencode "csrf_token=${login_csrf}"
)"
[[ "${login_code}" == "302" ]] || fail "valid login was rejected"
pass "password authentication and CSRF-protected login work"

about_page="$(
    curl_app --cookie "${COOKIE_JAR}" "${BASE_URL}/about"
)"
for required_text in \
    "Raspberry Pi OMT Client" \
    "vtest" \
    "Copyright (c) 2026 Matthew David Miller" \
    "MIT License" \
    "THIRD-PARTY SOFTWARE NOTICES" \
    "Open Media Transport"; do
    grep -Fq "${required_text}" <<<"${about_page}" ||
        fail "About page is missing: ${required_text}"
done
pass "About page renders version, copyright, license, and third-party notices"

system_page="$(
    curl_app --cookie "${COOKIE_JAR}" "${BASE_URL}/system"
)"
grep -Fq "Reboot operating system" <<<"${system_page}" ||
    fail "System page does not expose the reboot action"
confirm_page="$(
    curl_app --cookie "${COOKIE_JAR}" --cookie-jar "${COOKIE_JAR}" \
        "${BASE_URL}/system/reboot"
)"
reboot_csrf="$(printf '%s' "${confirm_page}" | csrf_from)"
[[ -n "${reboot_csrf}" ]] || fail "reboot confirmation CSRF token is missing"
grep -Fq "The Raspberry Pi will stop playback and go offline." <<<"${confirm_page}" ||
    fail "reboot confirmation warning is missing"
pass "System page requires an explicit reboot confirmation"

# Emulate only the host helper acknowledgement. host-reboot.sh itself is covered
# by unit contract tests and invokes the fixed Alpine reboot command only on Pi.
# shellcheck disable=SC2016
"${CONTAINER_ENGINE}" run -d \
    --name "${ACK_CONTAINER_NAME}" \
    -v "${actions_volume}" \
    --entrypoint /bin/sh "${IMAGE_TAG}" \
    -c '
        for attempt in $(seq 1 100); do
            if [ -s /host-actions/reboot.request ]; then
                request_id=$(sed -n "s/^request_id=//p" /host-actions/reboot.request)
                if [ "$(wc -l < /host-actions/reboot.request)" -eq 4 ] &&
                   grep -Fxq "version=1" /host-actions/reboot.request &&
                   grep -Fxq "action=reboot" /host-actions/reboot.request &&
                   printf "%s" "${request_id}" | grep -Eq "^[0-9a-f]{32}$"; then
                    printf "version=1\nrequest_id=%s\nstatus=accepted\ndetail=smoke-test\n" \
                        "${request_id}" > /host-actions/reboot.result
                    exit 0
                fi
            fi
            sleep 0.05
        done
        exit 1
    ' >/dev/null || fail "reboot acknowledgement emulator failed to start"

reboot_body="${TEST_ROOT}/reboot-response"
reboot_code="$(
    curl_app --output "${reboot_body}" --write-out '%{http_code}' \
        --cookie "${COOKIE_JAR}" \
        --request POST "${BASE_URL}/system/reboot" \
        --data-urlencode "csrf_token=${reboot_csrf}"
)"
if [[ "${reboot_code}" != "202" ]]; then
    printf '%s\n' "Reboot response body:" >&2
    sed -n '1,120p' "${reboot_body}" >&2
    fail "accepted reboot request did not return HTTP 202"
fi
grep -Fq "OS reboot scheduled" "${reboot_body}" ||
    fail "accepted reboot response is missing its offline notice"
timeout 10 "${CONTAINER_ENGINE}" wait "${ACK_CONTAINER_NAME}" >/dev/null 2>&1 ||
    fail "reboot acknowledgement emulator did not finish"
pass "CSRF-protected reboot request receives a correlated host acknowledgement"

manifest_check="$(
    "${CONTAINER_ENGINE}" exec "${CONTAINER_NAME}" \
        sha256sum --check /app/runtime-sha256.manifest 2>&1
)" || fail "runtime integrity manifest failed: ${manifest_check}"
pass "runtime integrity manifest remains valid after Web GUI operation"

echo "==============================="
echo -e "${GREEN}Native OMT container smoke tests passed!${NC}"
