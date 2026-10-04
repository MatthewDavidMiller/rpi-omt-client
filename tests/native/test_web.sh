#!/bin/bash
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
#
# Black-box contract for omt-web over real HTTPS: the routes, cookies, CSRF,
# rate limits, security headers, TLS versions, connection handling, the
# absent playout delay, password rotation, and the support bundle.
#
# Usage: tests/native/test_web.sh /path/to/omt-web
set -euo pipefail

WEB="$1"
ROOT="$(mktemp -d)"
PORT="$((20000 + RANDOM % 20000))"
URL="https://127.0.0.1:${PORT}"
failures=0
web_pid=""

cleanup() {
    [[ -n "${web_pid}" ]] && kill "${web_pid}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "${ROOT}"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    failures=$((failures + 1))
}

expect_eq() {
    [[ "$1" == "$2" ]] || fail "$3: expected '$2', got '$1'"
}

mkdir -p "${ROOT}/ssl" "${ROOT}/omt" "${ROOT}/run" "${ROOT}/host"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-384 -nodes -days 30 \
    -subj /CN=omt-client -keyout "${ROOT}/ssl/key.pem" -out "${ROOT}/ssl/cert.pem" 2>/dev/null
printf '0.9.test\n' > "${ROOT}/version"
printf 'Copyright (c) 2026 Matthew David Miller\n' > "${ROOT}/LICENSE"
printf 'notices <b>escaped</b>\n' > "${ROOT}/NOTICES"
: > "${ROOT}/host/request"
: > "${ROOT}/host/reboot.request"
cat > "${ROOT}/control" <<'EOF'
#!/bin/sh
echo "control $1"
exit 0
EOF
cat > "${ROOT}/receiver" <<'EOF'
#!/bin/sh
case "$1" in
    --version) echo "vtest" ;;
    discover) echo '[{"name":"Camera B","target":"Camera B","kind":"discovered"},{"name":"Camera A","target":"Camera A","kind":"discovered"}]' ;;
    probe) echo '{"ok":false,"error":"none"}'; exit 3 ;;
esac
EOF
chmod +x "${ROOT}/control" "${ROOT}/receiver"

export OMT_CONFIG_DIR="${ROOT}" WEB_PORT="${PORT}"
export OMT_CONTROL_COMMAND="${ROOT}/control" OMT_RECEIVER_COMMAND="${ROOT}/receiver"
export RPI_OMT_CLIENT_VERSION_FILE="${ROOT}/version"
export OMT_PROJECT_LICENSE_FILE="${ROOT}/LICENSE" OMT_THIRD_PARTY_NOTICES_FILE="${ROOT}/NOTICES"
export OMT_DIAGNOSTICS_HOST_REQUEST_FILE="${ROOT}/host/request"
export OMT_DIAGNOSTICS_HOST_REPORT_FILE="${ROOT}/host/report"
export OMT_DIAGNOSTICS_HOST_PCAP_FILE="${ROOT}/host/pcap"
export OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE="${ROOT}/host/pcap.txt"
export OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS=1 OMT_DIAGNOSTICS_RECEIVE_PROBE=0
export OMT_REBOOT_REQUEST_FILE="${ROOT}/host/reboot.request"
export OMT_REBOOT_RESULT_FILE="${ROOT}/host/reboot.result" OMT_REBOOT_ACK_TIMEOUT_SECONDS=0.3
export OMT_RUNTIME_INTEGRITY_MANIFEST="${ROOT}/manifest"
export OMT_LOGIN_RATE_LIMIT="4 per minute"

password="$("${WEB}" initialize)"
[[ "${password}" =~ ^[0-9a-f]{32}$ ]] || fail "initialize did not print a generated password"
[[ -z "$("${WEB}" initialize)" ]] || fail "a second initialize must not replace the password"
[[ "$(stat -c %a "${ROOT}/web_password")" == 600 ]] || fail "the password file must be 0600"
grep -Eq '^pbkdf2:sha256:600000\$[0-9a-f]{32}\$[0-9a-f]{64}$' "${ROOT}/web_password" ||
    fail "the password must be stored as a PBKDF2-SHA256 hash"

"${WEB}" >"${ROOT}/log" 2>&1 &
web_pid=$!
for _ in $(seq 1 100); do
    curl -sk -o /dev/null "${URL}/login" && break
    sleep 0.1
done

jar="${ROOT}/jar"
c() { curl -sk -c "${jar}" -b "${jar}" "$@"; }
token_of() { grep -oE 'name="csrf_token" value="[0-9a-f]{64}"' "$1" | head -n1 | sed 's/.*value="//;s/"$//'; }
status() { c -o "${ROOT}/body" -w '%{http_code}' "$@"; }

# Security headers on every response, including the unauthenticated login.
headers="$(c -D - -o /dev/null "${URL}/login")"
for expected in \
    "strict-transport-security: max-age=31536000; includeSubDomains" \
    "x-frame-options: DENY" \
    "x-content-type-options: nosniff" \
    "referrer-policy: strict-origin-when-cross-origin" \
    "content-security-policy: default-src 'self'; style-src 'self'; script-src 'none'; form-action 'self'" \
    "cache-control: no-store"; do
    grep -Fqi "${expected}" <<<"${headers}" || fail "missing header: ${expected}"
done
grep -Eqi '^set-cookie: __Host-omt_login=[0-9a-f]{48}; Path=/; Max-Age=600; Secure; HttpOnly; SameSite=Lax' <<<"${headers}" ||
    fail "the login nonce cookie is missing or malformed"
grep -Fqi "cache-control: public, max-age=86400" <<<"$(c -D - -o /dev/null "${URL}/static/style.css")" ||
    fail "static assets must be cacheable"
expect_eq "$(status "${URL}/static/favicon.svg")" 200 "favicon"

# Unauthenticated access.
expect_eq "$(status "${URL}/")" 303 "the dashboard redirects to login"
expect_eq "$(status "${URL}/nope")" 404 "an unknown page"
expect_eq "$(status "${URL}/logout")" 405 "GET on a POST-only route"
expect_eq "$(c -o /dev/null -w '%{http_code}' -I "${URL}/login")" 200 "HEAD on a GET route"

# Login: a missing or wrong CSRF token, then a wrong password, then success.
status "${URL}/login" >/dev/null
token="$(token_of "${ROOT}/body")"
expect_eq "$(status --data "csrf_token=wrong&password=${password}" "${URL}/login")" 400 "login CSRF"
status "${URL}/login" >/dev/null
token="$(token_of "${ROOT}/body")"
expect_eq "$(status --data "csrf_token=${token}&password=wrong" "${URL}/login")" 200 "wrong password"
grep -Fq "Invalid password" "${ROOT}/body" || fail "a wrong password must say so"
token="$(token_of "${ROOT}/body")"
expect_eq "$(status --data "csrf_token=${token}&password=${password}" "${URL}/login")" 303 "login"
grep -Eq '__Host-omt_session[[:space:]]+[0-9a-f]{64}' "${jar}" || fail "no session cookie"

# Every authenticated page renders.
for page in / /settings/network /diagnostics /system /system/reboot /about; do
    expect_eq "$(status "${URL}${page}")" 200 "page ${page}"
done
status "${URL}/" >/dev/null
grep -Fq "Camera A — OMT discovery" "${ROOT}/body" || fail "discovered sources are listed"
status "${URL}/about" >/dev/null
grep -Fq "notices &lt;b&gt;escaped&lt;&#x2f;b&gt;" "${ROOT}/body" || fail "template output must be HTML-escaped"
grep -Fq "0.9.test" "${ROOT}/body" || fail "the version is shown"
token="$(token_of "${ROOT}/body")"

# A POST without the session's token is refused.
expect_eq "$(status --data "csrf_token=wrong" "${URL}/sources/refresh")" 400 "session CSRF"
expect_eq "$(status --data "csrf_token=${token}" "${URL}/sources/refresh")" 303 "refresh"

# Source selection persists the strict record.
expect_eq "$(status --data-urlencode "csrf_token=${token}" --data-urlencode "source=discovered|Camera A" "${URL}/sources/select")" 303 "select"
expect_eq "$(cat "${ROOT}/source_target.json")" '{"schema":1,"kind":"discovered","name":"Camera A"}' "saved source"
expect_eq "$("${WEB}" play-target "${ROOT}/source_target.json")" "Camera A" "play-target"
status "${URL}/" >/dev/null
grep -Fq "Camera A saved" "${ROOT}/body" && fail "flash should name the backend, not the source"
grep -Fq "OMT discovery saved and running." "${ROOT}/body" || fail "the flash message appears once"
status "${URL}/" >/dev/null
grep -Fq "OMT discovery saved and running." "${ROOT}/body" && fail "a flash is shown only once"

# There is no playout delay: no route sets one, no page offers one, and the
# helper the launcher once called is gone.
expect_eq "$(status --data "csrf_token=${token}&playout_delay=250" "${URL}/system/playout-delay")" 404 "no delay route"
[[ ! -e "${ROOT}/playout_delay.json" ]] || fail "a playout delay was saved"
status "${URL}/system" >/dev/null
grep -Fqi "playout" "${ROOT}/body" && fail "the System page still offers a playout delay"
"${WEB}" playout-delay "${ROOT}/playout_delay.json" 0 >/dev/null 2>&1 && fail "omt-web playout-delay still exists"

# Video limit and network settings.
expect_eq "$(status --data "csrf_token=${token}&video_limit=1920x1080%4030,1280x720%4060" "${URL}/system/video-limit")" 303 "video limit"
expect_eq "$("${WEB}" video-ceiling "${ROOT}/video_ceiling.json" 1920x1080@60)" "1920x1080@30,1280x720@60" "ceiling"
expect_eq "$(status --data "csrf_token=${token}&discovery_server=Example.COM" "${URL}/settings/network")" 303 "discovery server"
grep -Fq "<DiscoveryServer>omt://example.com:6399</DiscoveryServer>" "${ROOT}/omt/settings.xml" ||
    fail "the discovery server is normalized and saved"
expect_eq "$(status --data "csrf_token=${token}&discovery_server=2001:db8::1" "${URL}/settings/network")" 200 "bad server"
grep -Fq "must be bracketed" "${ROOT}/body" || fail "a refused server explains itself"

# Diagnostics, including the support bundle.
expect_eq "$(status --data "csrf_token=${token}" "${URL}/diagnostics/runtime")" 200 "runtime check"
grep -Fq "OMT runtime checks" "${ROOT}/body" || fail "the runtime check renders"
c -o "${ROOT}/bundle.zip" -D "${ROOT}/bundle.headers" --data "csrf_token=${token}" "${URL}/diagnostics/download"
grep -Fqi "content-type: application/zip" "${ROOT}/bundle.headers" || fail "the bundle is a zip"
grep -Eqi 'content-disposition: attachment; filename="omt-diagnostics-[0-9]{8}T[0-9]{6}Z.zip"' "${ROOT}/bundle.headers" ||
    fail "the bundle is named by its UTC timestamp"
python3 - "${ROOT}/bundle.zip" <<'EOF' || fail "the support bundle is not a valid ZIP"
import sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    assert z.testzip() is None
    names = z.namelist()
    for required in ("version.txt", "runtime-settings.txt", "runtime.txt", "discovery.json",
                     "controller-status.txt", "current-target-receive-probe.json",
                     "playback-status.json", "omt-settings.xml", "runtime-sha256.manifest",
                     "host-report.txt", "host-network-pcap.txt"):
        assert required in names, required
    assert z.getinfo("version.txt").compress_type == zipfile.ZIP_DEFLATED
    assert z.read("version.txt") == b"0.9.test\n"
    assert z.read("host-report.txt").startswith(b"unavailable: ")
EOF

# Reboot: the host bridge never answers, so the request is refused clearly.
expect_eq "$(status --data "csrf_token=${token}" "${URL}/system/reboot")" 303 "unacknowledged reboot"

# A slow client cannot hold the server: an idle TLS connection stays open
# while another client is served.
(sleep 3 | openssl s_client -quiet -connect "127.0.0.1:${PORT}" >/dev/null 2>&1) &
sleep 0.3
expect_eq "$(status "${URL}/system")" 200 "served beside an idle connection"

# Bodies past OMT_MAX_REQUEST_BYTES are refused before a handler runs.
head -c 20000 /dev/zero | tr '\0' 'a' > "${ROOT}/big"
expect_eq "$(status --data-binary "@${ROOT}/big" "${URL}/login")" 413 "oversized body"

# TLS: 1.2 and 1.3 only.
openssl s_client -connect "127.0.0.1:${PORT}" -tls1_3 </dev/null >/dev/null 2>&1 || fail "TLS 1.3 refused"
openssl s_client -connect "127.0.0.1:${PORT}" -tls1_2 </dev/null >/dev/null 2>&1 || fail "TLS 1.2 refused"
if openssl s_client -connect "127.0.0.1:${PORT}" -tls1_1 </dev/null >/dev/null 2>&1; then
    fail "TLS 1.1 accepted"
fi

# Logout revokes the session.
expect_eq "$(status --data "csrf_token=${token}" "${URL}/logout")" 303 "logout"
expect_eq "$(status "${URL}/system")" 303 "a revoked session is refused"

# Rate limit: 4 per minute, and three attempts were already spent above.
codes=""
for _ in 1 2 3; do
    status "${URL}/login" >/dev/null
    t="$(token_of "${ROOT}/body")"
    codes+="$(status --data "csrf_token=${t}&password=wrong" "${URL}/login") "
done
[[ "${codes}" == *429* ]] || fail "login attempts are rate limited: ${codes}"

# Rotating the password ends every session and the old password.
kill "${web_pid}"
wait "${web_pid}" 2>/dev/null || true
printf 'a new password value\n' | "${WEB}" set-password >/dev/null || fail "set-password"
if printf 'short\n' | "${WEB}" set-password >/dev/null 2>&1; then fail "a short password was accepted"; fi
grep -Fq "600000" "${ROOT}/web_password" || fail "set-password stores a PBKDF2 hash"

if ((failures)); then
    echo "omt-web contract: ${failures} failure(s)" >&2
    cat "${ROOT}/log" >&2
    exit 1
fi
echo "omt-web HTTPS contracts passed"
