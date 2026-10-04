/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Deployment, management, and Wi-Fi operations over an SSH session.
 */
#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "crypto/crypto.h"
#include "deploy/core/deploy.h"
#include "deploy/core/ops_internal.h"
#include "deploy/ssh/ssh.h"

#define PLATFORM_PROBE                                                                             \
    "uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && cat /etc/alpine-release && tr "    \
    "-d '\\000' < /proc/device-tree/model && printf '\\n'"
#define BOOTSTRAP_PASSWORD_READY "omt-bootstrap-password-ready"
#define SETUP_SYS_MEMBER DP_SETUP_SYS_MEMBER
#define SETUP_SYS_COMPLETE DP_SETUP_SYS_COMPLETE
#define SET_HOSTNAME_MEMBER DP_SET_HOSTNAME_MEMBER
#define SET_HOSTNAME_COMPLETE DP_SET_HOSTNAME_COMPLETE

const char DP_WEB_PASSWORD_COMMAND[] =
    "sh -eu -c 'docker exec -i omt-client /usr/local/bin/omt-web set-password && rc-service "
    "omt-client restart'";

/* The appliance's 5 GHz band policy, as wpa_supplicant's freq_list.
 *
 * Real-world testing settled that 2.4 GHz cannot carry an OMT stream: the loss
 * rate makes playback unusable however strong the signal is. Keep this
 * identical to HOST_WIFI_FREQ_LIST in deploy/lib/service-install.sh and
 * WIFI_FREQ_LIST in deploy/host/setup-sys.sh; the three cannot share a
 * definition because they run on a workstation, an installed appliance, and a
 * factory image respectively. tests/unit/test_setup_sys.sh compares them. */
#define WIFI_FREQ_LIST                                                                             \
    "5180 5200 5220 5240 5260 5280 5300 5320 5500 5520 5540 5560 5580 5600 5620 5640 5660 5680 "   \
    "5700 5720 5745 5765 5785 5805 5825"

/* clang-format off */
const char DP_WIFI_SCRIPT[] =
    "marker=$4\n"
    "found_marker=no\n"
    "while IFS= read -r line; do\n"
    "  if [ \"$line\" = \"$marker\" ]; then found_marker=yes; break; fi\n"
    "done\n"
    "if [ \"$found_marker\" != yes ]; then echo \"Wi-Fi password marker not found\" >&2; exit 11; fi\n"
    "if ! IFS= read -r wifi_password; then echo \"Wi-Fi password not provided\" >&2; exit 11; fi\n"
    "ssid_hex=$1\n"
    "activate=$2\n"
    "preserve=$3\n"
    "ssid_text=$5\n"
    "config_path=/etc/wpa_supplicant/wpa_supplicant.conf\n"
    "secure_saved_config() {\n"
    "  [ -f \"$config_path\" ] && [ ! -L \"$config_path\" ] || { echo 'Unsafe wpa_supplicant configuration path' >&2; exit 15; }\n"
    "  chown root:root \"$config_path\"\n"
    "  chmod 600 \"$config_path\"\n"
    "}\n"
    "command -v wpa_cli >/dev/null 2>&1 || { echo 'wpa_cli is unavailable' >&2; exit 12; }\n"
    "iface=\n"
    "if command -v iw >/dev/null 2>&1; then\n"
    "  iface=$(iw dev 2>/dev/null | awk '$1 == \"Interface\" { print $2; exit }')\n"
    "fi\n"
    "if [ -z \"$iface\" ]; then\n"
    "  for path in /sys/class/net/*/wireless; do\n"
    "    [ -e \"$path\" ] || continue\n"
    "    iface=${path#/sys/class/net/}\n"
    "    iface=${iface%/wireless}\n"
    "    break\n"
    "  done\n"
    "fi\n"
    "[ -n \"$iface\" ] || iface=wlan0\n"
    "wpa_cli -i \"$iface\" ping | grep -Fxq PONG || { echo \"wpa_supplicant is unavailable on $iface\" >&2; exit 12; }\n"
    "secure_saved_config\n"
    /* Establish the regulatory country before scanning, defaulting to US.
     * Without one the radio is in the world domain, which has no channels
     * 149-165 at all -- so a scan run first cannot see the 5 GHz access points
     * most US sites use, and the operator is offered 2.4 GHz networks only. A
     * country the board already declares is left alone; it is where the
     * appliance actually is, and managing Wi-Fi must not relabel the radio. */
    "country=$(wpa_cli -i \"$iface\" get country 2>/dev/null || true)\n"
    "case \"$country\" in\n"
    "  [A-Z][A-Z]) ;;\n"
    "  *) wpa_cli -i \"$iface\" set country US >/dev/null 2>&1 || true;;\n"
    "esac\n"
    /* Ask supplicant to narrow the scan where its build supports the global
     * control. Alpine currently returns FAIL for this spelling, so the
     * per-network setting below is the authoritative enforcement point. */
    "wpa_cli -i \"$iface\" set freq_list \"" WIFI_FREQ_LIST "\" >/dev/null 2>&1 || true\n"
    "wpa_cli -i \"$iface\" scan >/dev/null || true\n"
    "network_id=\n"
    "if [ \"$preserve\" = yes ]; then\n"
    "  network_id=$(wpa_cli -i \"$iface\" list_networks | SSID_TARGET=\"$ssid_text\" awk -F '\\t' 'NR > 1 && $2 == ENVIRON[\"SSID_TARGET\"] { print $1; exit }')\n"
    "fi\n"
    "if [ -z \"$network_id\" ]; then network_id=$(wpa_cli -i \"$iface\" add_network); fi\n"
    "case \"$network_id\" in ''|*[!0-9]*) echo 'Unable to allocate Wi-Fi profile' >&2; exit 13;; esac\n"
    "wpa_cli -i \"$iface\" set_network \"$network_id\" ssid \"$ssid_hex\" | grep -Fxq OK\n"
    "wpa_cli -i \"$iface\" set_network \"$network_id\" key_mgmt WPA-PSK | grep -Fxq OK\n"
    "wpa_cli -i \"$iface\" set_network \"$network_id\" psk \"$wifi_password\" | grep -Fxq OK\n"
    "wpa_cli -i \"$iface\" set_network \"$network_id\" freq_list \"" WIFI_FREQ_LIST "\" | grep -Fxq OK\n"
    "unset wifi_password\n"
    "if [ \"$activate\" = no ]; then\n"
    "  wpa_cli -i \"$iface\" disconnect | grep -Fxq OK\n"
    "fi\n"
    "wpa_cli -i \"$iface\" enable_network \"$network_id\" | grep -Fxq OK\n"
    "if [ \"$preserve\" = yes ]; then\n"
    "  wpa_cli -i \"$iface\" save_config | grep -Fxq OK\n"
    "  secure_saved_config\n"
    "fi\n"
    "if [ \"$activate\" = yes ]; then\n"
    "  wpa_cli -i \"$iface\" select_network \"$network_id\" >/dev/null\n"
    "  wpa_cli -i \"$iface\" reassociate >/dev/null\n"
    "fi\n"
    "if [ \"$preserve\" = no ] && [ \"$activate\" = yes ]; then\n"
    "  association_ready=no\n"
    "  attempt=0\n"
    "  while [ \"$attempt\" -lt 30 ]; do\n"
    "    status=$(wpa_cli -i \"$iface\" status 2>/dev/null || true)\n"
    "    status_id=$(printf '%s\\n' \"$status\" | awk -F= '$1 == \"id\" { print $2; exit }')\n"
    "    status_state=$(printf '%s\\n' \"$status\" | awk -F= '$1 == \"wpa_state\" { print $2; exit }')\n"
    "    if [ \"$status_id\" = \"$network_id\" ] && [ \"$status_state\" = COMPLETED ]; then association_ready=yes; break; fi\n"
    "    attempt=$((attempt + 1))\n"
    "    sleep 1\n"
    "  done\n"
    "  if [ \"$association_ready\" != yes ]; then\n"
    "    wpa_cli -i \"$iface\" reconfigure >/dev/null 2>&1 || true\n"
    "    echo 'New Wi-Fi profile did not associate; existing profiles were retained' >&2\n"
    "    exit 14\n"
    "  fi\n"
    "fi\n"
    "if [ \"$preserve\" = no ]; then\n"
    "  for candidate in $(wpa_cli -i \"$iface\" list_networks | awk 'NR > 1 {print $1}'); do\n"
    "    [ \"$candidate\" = \"$network_id\" ] && continue\n"
    "    wpa_cli -i \"$iface\" remove_network \"$candidate\" | grep -Fxq OK\n"
    "  done\n"
    "  wpa_cli -i \"$iface\" save_config | grep -Fxq OK\n"
    "  secure_saved_config\n"
    "fi\n"
    "command -v iw >/dev/null 2>&1 && iw dev \"$iface\" set power_save off || true\n";
/* clang-format on */

/* Where one capsule member's bytes come from: compiled in, where nothing can
 * change them between the digest and the upload, or a file in a developer's
 * working tree, which is re-verified once its upload completes. */
typedef struct {
    const uint8_t *bytes; /* embedded when non-NULL */
    size_t size;
    char *path;
    char *fingerprint;
} payload;

typedef struct {
    char *name;
    char digest[65];
    payload data;
} artifact;

static void payload_free(payload *p) {
    free(p->path);
    free(p->fingerprint);
    p->path = p->fingerprint = NULL;
}

static bool check_cancel(const dp_cancel *cancel, dp_err *err) {
    if (!dp_cancelled(cancel)) return true;
    dp_fail_cancelled(err);
    return false;
}

static void text_init(omt_buf *b) { omt_buf_init(b, DP_ERR_LIMIT); }

/* Appends ":\n<detail>" unless the detail is only whitespace. */
static void fail_with_detail(dp_err *err, const char *what, const char *detail) {
    omt_span trimmed = omt_utf8_trim(detail, strlen(detail));
    if (trimmed.len == 0) {
        dp_fail(err, "%s", what);
    } else {
        dp_fail(err, "%s:\n%s", what, detail);
    }
}

bool dp_require_success(const ssh_result *result, const char *operation, dp_err *err) {
    if (ssh_result_success(result)) return true;
    omt_buf detail, what;
    text_init(&detail);
    text_init(&what);
    ssh_result_combined(result, &detail);
    omt_buf_printf(&what, "%s failed", operation);
    fail_with_detail(err, omt_buf_cstr(&what), omt_buf_cstr(&detail));
    omt_buf_free(&detail);
    omt_buf_free(&what);
    return false;
}

const char *dp_sudo_prefix(const dp_connection *c) {
    if (strcmp(c->username, "root") == 0) return "";
    if (dp_secret_nonempty(&c->sudo_password)) return "sudo -S -p ''";
    return "sudo -n";
}

void dp_privileged_command(const dp_connection *c, const char *command, omt_buf *out) {
    const char *sudo = dp_sudo_prefix(c);
    if (sudo[0]) {
        omt_buf_puts(out, sudo);
        omt_buf_putc(out, ' ');
    }
    omt_buf_puts(out, command);
}

/* Keep authentication and execution in one process so the remaining stdin is
 * delivered to the command after sudo consumes its password. */
void dp_privileged_stdin_command(const dp_connection *c, const char *command, omt_buf *out) {
    dp_privileged_command(c, command, out);
}

void dp_privileged_argv_command(const dp_connection *c, const char *const *argv, omt_buf *out) {
    omt_buf joined;
    text_init(&joined);
    for (size_t i = 0; argv[i]; i++) {
        if (i) omt_buf_putc(&joined, ' ');
        dp_shell_quote(&joined, argv[i]);
    }
    dp_privileged_command(c, omt_buf_cstr(&joined), out);
    omt_buf_free(&joined);
}

/* The sudo password as the remote shell expects it on stdin, in a buffer that
 * is wiped when freed: deploy holds it for the whole upload-verify-promote-
 * install sequence. */
void dp_sudo_input(const dp_connection *c, omt_buf *out) {
    if (strcmp(c->username, "root") == 0 || !dp_secret_nonempty(&c->sudo_password)) return;
    omt_buf_append(out, dp_secret_text(&c->sudo_password), dp_secret_len(&c->sudo_password));
    omt_buf_putc(out, '\n');
}

static void secret_buf_init(omt_buf *b) { omt_buf_init(b, 64u * 1024u); }

/* ------------------------------------------------------------- tooling */

/* Reports the deploy account's uid, then bash, sudo, and doas as yes/no.
 *
 * doas is reported by whether it can escalate, not by whether it exists.
 * Alpine ships the binary on every image with each rule in /etc/doas.conf
 * commented out, so presence proved nothing and bootstrap_escalation picked
 * doas on exactly the stock hosts where it cannot work. /etc/doas.conf is mode
 * 0640 root:root, so the deploy account cannot read it to find out either. Ask
 * doas instead: a rule that matched but wants a password reports an
 * authentication failure and will succeed once a password is supplied, while
 * an unmatched rule reports that the operation is not permitted. */
static const char TOOLING_PROBE[] =
    "id -u; for tool in bash sudo; do "
    "if command -v \"$tool\" >/dev/null 2>&1; then echo yes; else echo no; fi; done; "
    "if ! command -v doas >/dev/null 2>&1; then echo no; "
    "elif doas -n true >/dev/null 2>&1; then echo yes; "
    "else case \"$(doas -n true 2>&1)\" in "
    "*[Aa]uthenticat*|*[Aa]uthoriz*) echo yes ;; *) echo no ;; esac; fi";

/* The n-th line (from 0) of text, as str::lines splits it, without the line
 * ending. Returns false when there is no such line. */
static bool nth_line(const char *text, size_t n, const char **start, size_t *len) {
    const char *p = text;
    for (size_t i = 0;; i++) {
        if (!*p) return false;
        const char *nl = strchr(p, '\n');
        size_t l = nl ? (size_t)(nl - p) : strlen(p);
        if (i == n) {
            *start = p;
            *len = l > 0 && p[l - 1] == '\r' ? l - 1 : l;
            return true;
        }
        if (!nl) return false;
        p = nl + 1;
    }
}

static bool line_trimmed_is(const char *text, size_t n, const char *value, char *copy,
                            size_t copy_size) {
    const char *start;
    size_t len;
    if (!nth_line(text, n, &start, &len)) {
        if (copy) copy[0] = 0;
        return value[0] == 0;
    }
    omt_span t = omt_utf8_trim(start, len);
    if (copy) omt_snprintf(copy, copy_size, "%.*s", (int)t.len, (const char *)t.p);
    return t.len == strlen(value) && memcmp(t.p, value, t.len) == 0;
}

dp_host_tooling dp_parse_host_tooling(const char *output) {
    dp_host_tooling tooling;
    line_trimmed_is(output, 0, "", tooling.uid, sizeof(tooling.uid));
    tooling.has_bash = line_trimmed_is(output, 1, "yes", NULL, 0);
    tooling.has_sudo = line_trimmed_is(output, 2, "yes", NULL, 0);
    tooling.has_doas = line_trimmed_is(output, 3, "yes", NULL, 0);
    return tooling;
}

const char *dp_bootstrap_escalation(const dp_host_tooling *tooling, dp_err *err) {
    if (strcmp(tooling->uid, "0") == 0) return "";
    if (tooling->has_sudo) return "sudo -S -p ''";
    if (tooling->has_doas) return "doas";
    dp_fail(err,
            "this Raspberry Pi has no sudo, no doas, and the deploy account is not root, so the "
            "appliance cannot be bootstrapped remotely. Alpine ships neither by default. Fill the "
            "Alpine view's root password (`bootstrap_root_password` in the CLI `--secrets-stdin`) "
            "and the first deploy installs them through `su`. Otherwise connect as root, or run "
            "`su -c '/bin/sh bootstrap.sh'` once on the Pi with deploy/host/bootstrap.sh copied "
            "across.");
    return NULL;
}

bool dp_needs_su_bootstrap(const dp_host_tooling *tooling, const dp_connection *c) {
    return strcmp(tooling->uid, "0") != 0 && !tooling->has_sudo &&
           dp_secret_nonempty(&c->bootstrap_root_password);
}

/* ---------------------------------------------------------- the capsule */

static bool tree_payload(const char *root, const char *name, payload *out, dp_err *err) {
    omt_buf path, fp;
    text_init(&path);
    text_init(&fp);
    bool ok = dp_secure_relative(root, name, &path, err) &&
              dp_file_fingerprint(omt_buf_cstr(&path), &fp, err);
    if (ok) {
        out->bytes = NULL;
        out->path = dp_strdup(omt_buf_cstr(&path));
        out->fingerprint = dp_strdup(omt_buf_cstr(&fp));
        ok = out->path && out->fingerprint;
    }
    omt_buf_free(&path);
    omt_buf_free(&fp);
    return ok;
}

/* A single member, for the scripts uploaded outside a staged deployment. */
static bool capsule_member(const char *root, const char *name, payload *out, dp_err *err) {
    memset(out, 0, sizeof(*out));
    if (root) return tree_payload(root, name, out, err);
    const dp_capsule_member *m = dp_capsule_member_named(name);
    if (!m) {
        dp_fail(err, "%s is not part of the embedded capsule", name);
        return false;
    }
    out->bytes = m->bytes;
    out->size = m->size;
    return true;
}

static void artifacts_free(artifact *a, size_t n) {
    if (!a) return;
    for (size_t i = 0; i < n; i++) {
        free(a[i].name);
        payload_free(&a[i].data);
    }
    free(a);
}

/* Every member this deployment uploads, in the order it sends them. A project
 * root replaces the whole capsule rather than any part of it: mixing this
 * binary's host scripts with a working tree's image would deploy a
 * combination nobody built. */
static bool capsule_artifacts(const dp_deploy_options *o, artifact **out, size_t *count,
                              dp_err *err) {
    *out = NULL;
    *count = 0;
    if (!o->project_root) {
        size_t n;
        const dp_capsule_member *members = dp_capsule_members(&n);
        artifact *a = calloc(n ? n : 1, sizeof(*a));
        if (!a) {
            dp_fail(err, "out of memory");
            return false;
        }
        for (size_t i = 0; i < n; i++) {
            a[i].name = dp_strdup(members[i].name);
            dp_sha256_hex(members[i].bytes, members[i].size, a[i].digest);
            a[i].data.bytes = members[i].bytes;
            a[i].data.size = members[i].size;
        }
        *out = a;
        *count = n;
        return true;
    }
    omt_buf path;
    text_init(&path);
    dp_path_join(&path, o->project_root, "deploy");
    omt_buf manifest_path;
    text_init(&manifest_path);
    dp_path_join(&manifest_path, omt_buf_cstr(&path), "manifest-v3.txt");
    omt_buf_free(&path);
    dp_manifest m;
    bool ok = dp_load_manifest(omt_buf_cstr(&manifest_path), &m, err);
    omt_buf_free(&manifest_path);
    if (!ok) return false;
    bool has_image = false;
    for (size_t i = 0; i < m.count; i++) has_image |= strcmp(m.names[i], DP_IMAGE_MEMBER) == 0;
    if (!has_image) {
        dp_manifest_free(&m);
        dp_fail(err, "deployment artifact manifest does not include " DP_IMAGE_MEMBER);
        return false;
    }
    artifact *a = calloc(m.count, sizeof(*a));
    if (!a) {
        dp_manifest_free(&m);
        dp_fail(err, "out of memory");
        return false;
    }
    size_t done = 0;
    for (; ok && done < m.count; done++) {
        a[done].name = dp_strdup(m.names[done]);
        ok = tree_payload(o->project_root, m.names[done], &a[done].data, err) &&
             dp_sha256_file(a[done].data.path, a[done].digest, err);
    }
    dp_manifest_free(&m);
    if (!ok) {
        artifacts_free(a, done);
        return false;
    }
    *out = a;
    *count = done;
    return true;
}

/* What this deployment is about to send, named before it sends any of it. The
 * archive's digest is in the line because it is the one member an operator
 * cannot inspect. */
static void capsule_summary(const artifact *a, size_t n, const dp_deploy_options *o, omt_buf *out) {
    omt_buf origin, image;
    text_init(&origin);
    text_init(&image);
    if (o->project_root) {
        omt_buf_printf(&origin, "from %s", o->project_root);
    } else {
        omt_buf_puts(&origin, "embedded in this deployer");
    }
    omt_buf_puts(&image, "no " DP_IMAGE_MEMBER);
    for (size_t i = 0; i < n; i++) {
        if (strcmp(a[i].name, DP_IMAGE_MEMBER) != 0) continue;
        uint64_t bytes = a[i].data.bytes ? a[i].data.size : 0;
        if (!a[i].data.bytes && !dp_file_size(a[i].data.path, &bytes)) bytes = 0;
        omt_buf_clear(&image);
        omt_buf_printf(&image, DP_IMAGE_MEMBER " %llu MiB, sha256 %.12s",
                       (unsigned long long)(bytes / (1024u * 1024u)), a[i].digest);
    }
    omt_buf_printf(out, "Deploying the manifest-v3 capsule %s: %zu files, %s.",
                   omt_buf_cstr(&origin), n, omt_buf_cstr(&image));
    omt_buf_free(&origin);
    omt_buf_free(&image);
}

static bool upload_payload(ssh_session *s, const payload *p, const char *remote,
                           const dp_cancel *cancel, dp_err *err) {
    if (p->bytes) return ssh_upload_bytes(s, p->bytes, p->size, remote, cancel, err);
    return ssh_upload_file(s, p->path, remote, cancel, err);
}

/* Whether an artifact is still what it was when its digest was taken. Always
 * true for the embedded capsule: those bytes are part of the running program. */
static bool artifact_unchanged(const artifact *a, bool *unchanged, dp_err *err) {
    *unchanged = true;
    if (a->data.bytes) return true;
    omt_buf fp;
    text_init(&fp);
    char digest[65];
    bool ok =
        dp_file_fingerprint(a->data.path, &fp, err) && dp_sha256_file(a->data.path, digest, err);
    if (ok) {
        *unchanged =
            strcmp(omt_buf_cstr(&fp), a->data.fingerprint) == 0 && strcmp(digest, a->digest) == 0;
    }
    omt_buf_free(&fp);
    return ok;
}

bool dp_parse_sha256_line(const char *output, char out[65]) {
    const char *p = output;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    size_t n = strcspn(p, " \t\r\n");
    if (n != 64) return false;
    for (size_t i = 0; i < 64; i++) {
        char c = p[i];
        bool hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
        if (!hex) return false;
        out[i] = (char)(c >= 'A' && c <= 'F' ? c + 32 : c);
    }
    out[64] = 0;
    return true;
}

/* ---------------------------------------------------------- the platform */

/* The boards this appliance supports, as device-tree model prefixes. This is
 * the C half of the table in deploy/lib/board-profile.sh; the installer
 * refuses the same set on the host itself. Each prefix ends at a word
 * boundary, which is why the Pi 5 entry is not simply matched loosely:
 * "Raspberry Pi 5" is also the start of "Raspberry Pi 500".
 *
 * Both entries have a dual-band radio, and that is a support criterion rather
 * than a coincidence. The appliance is 5 GHz only because real-world testing
 * showed 2.4 GHz packet loss makes OMT playback unusable, so a board that
 * cannot leave 2.4 GHz cannot be a host. That removed the Pi Zero 2 W
 * (BCM43436) and the Pi 3 tier, whose Model B (BCM43438) has no 5 GHz radio. */
static const char *const SUPPORTED_BOARDS[] = {"Raspberry Pi 5", "Raspberry Pi 4 Model B"};

bool dp_is_supported_board(const char *model, size_t len) {
    for (size_t i = 0; i < OMT_ARRAY_LEN(SUPPORTED_BOARDS); i++) {
        const char *prefix = SUPPORTED_BOARDS[i];
        size_t n = strlen(prefix);
        if (len < n || memcmp(model, prefix, n) != 0) continue;
        /* A prefix that already ends in a space has consumed its own
         * boundary; otherwise the next character must start a new word. */
        if (prefix[n - 1] == ' ' || len == n || model[n] == ' ') return true;
    }
    return false;
}

bool dp_require_supported_appliance(const char *output, dp_err *err) {
    const char *arch, *system, *release, *model;
    size_t al = 0, sl = 0, rl = 0, ml = 0;
    bool ok = nth_line(output, 0, &arch, &al) && nth_line(output, 1, &system, &sl) &&
              nth_line(output, 2, &release, &rl) && nth_line(output, 3, &model, &ml);
    ok = ok && al == 7 && memcmp(arch, "aarch64", 7) == 0 && sl == 6 &&
         memcmp(system, "alpine", 6) == 0 && rl >= 5 && memcmp(release, "3.24.", 5) == 0 &&
         dp_is_supported_board(model, ml);
    if (!ok) {
        dp_fail(err, "remote host must run Alpine Linux 3.24 aarch64 on a Raspberry Pi 5 or "
                     "Raspberry Pi 4 Model B");
    }
    return ok;
}

bool dp_probed_board(const char *output, char *out, size_t size) {
    const char *model;
    size_t len;
    if (!nth_line(output, 3, &model, &len) || len == 0) return false;
    omt_snprintf(out, size, "%.*s", (int)len, model);
    return true;
}

void dp_redact(const char *message, const char *const *secrets, size_t count, omt_buf *out) {
    omt_buf_clear(out);
    omt_buf_puts(out, message);
    static const char marker[] = "[redacted]";
    for (size_t s = 0; s < count; s++) {
        const char *secret = secrets[s];
        size_t n = secret ? strlen(secret) : 0;
        if (n == 0) continue;
        omt_buf next;
        text_init(&next);
        const char *p = omt_buf_cstr(out);
        for (;;) {
            const char *hit = strstr(p, secret);
            if (!hit) {
                omt_buf_puts(&next, p);
                break;
            }
            omt_buf_append(&next, p, (size_t)(hit - p));
            omt_buf_puts(&next, marker);
            p = hit + n;
        }
        omt_buf_clear(out);
        omt_buf_append(out, next.data, next.len);
        omt_buf_free_secret(&next);
    }
}

bool dp_installer_summary(const char *output, omt_buf *out) {
    const char *start = strstr(output, "=== Installation Complete ===");
    if (!start) return false;
    omt_span t = omt_utf8_trim(start, strlen(start));
    if (t.len == 0) return false;
    omt_buf_append(out, t.p, t.len);
    return true;
}

bool dp_first_web_password(const char *logs, omt_buf *out) {
    const char *p = logs;
    while (*p) {
        const char *nl = strchr(p, '\n');
        size_t len = nl ? (size_t)(nl - p) : strlen(p);
        bool banner = false;
        for (size_t i = 0; i + 31 <= len; i++) {
            if (memcmp(p + i, "Web UI password (save this now)", 31) == 0) banner = true;
        }
        if (!nl) return false;
        p = nl + 1;
        if (!banner) continue;
        const char *value;
        size_t vlen;
        if (!nth_line(p, 0, &value, &vlen)) return false;
        omt_span t = omt_utf8_trim(value, vlen);
        bool all_equals = true;
        for (size_t i = 0; i < t.len; i++) all_equals &= t.p[i] == '=';
        if (t.len == 0 || all_equals) return false;
        omt_buf_append(out, t.p, t.len);
        return true;
    }
    return false;
}

/* ------------------------------------------------------------- sessions */

static bool connect_checked(const dp_connection *c, const dp_cancel *cancel, ssh_session **out,
                            dp_err *err) {
    const char *problem = dp_validate_connection(c);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    return ssh_connect(c, cancel, out, err);
}

static bool run_simple(ssh_session *s, const char *command, const omt_buf *input,
                       const dp_cancel *cancel, ssh_result *r, dp_err *err) {
    ssh_result_free(r);
    ssh_result_init(r);
    return ssh_run(s, command, input ? (const char *)input->data : "", input ? input->len : 0,
                   cancel, r, err);
}

static bool read_boot_id(ssh_session *s, const dp_cancel *cancel, omt_buf *out, dp_err *err) {
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, "cat /proc/sys/kernel/random/boot_id", NULL, cancel, &r, err) &&
              dp_require_success(&r, "Read Linux boot ID", err);
    if (ok) {
        omt_span t = omt_utf8_trim((const char *)r.out.data, r.out.len);
        if (t.len == 0) {
            dp_fail(err, "Linux boot ID was empty");
            ok = false;
        } else {
            omt_buf_clear(out);
            omt_buf_append(out, t.p, t.len);
        }
    }
    ssh_result_free(&r);
    return ok;
}

/* Per connection, because these are different accounts on the same board and
 * they do not have to fail together. One shared flag would let a refusal on
 * one account stand in as proof that the host went away, and the next account
 * to answer would then return a session belonging to the pre-reboot system
 * that is still shutting down. */
static bool wait_for_reboot(const dp_connection *const *connections, size_t count,
                            const char *previous_boot_id, uint64_t timeout_ms,
                            const dp_cancel *cancel, const dp_progress *p, ssh_session **out,
                            dp_err *err) {
    dp_report(p, "Waiting for the Raspberry Pi to reboot...");
    uint64_t deadline = dp_now_ms() + timeout_ms;
    bool saw_down[4] = {false, false, false, false};
    omt_buf boot_id;
    text_init(&boot_id);
    dp_err attempt;
    dp_err_init(&attempt);
    bool found = false;
    while (!found && dp_now_ms() < deadline) {
        if (!check_cancel(cancel, err)) break;
        for (size_t i = 0; i < count && i < 4 && !found; i++) {
            ssh_session *s = NULL;
            if (!connect_checked(connections[i], cancel, &s, &attempt)) {
                saw_down[i] = true;
                continue;
            }
            bool read = read_boot_id(s, cancel, &boot_id, &attempt);
            if ((read && strcmp(omt_buf_cstr(&boot_id), previous_boot_id) != 0) ||
                (!read && saw_down[i])) {
                *out = s;
                found = true;
            } else {
                ssh_close(s);
            }
        }
        if (!found) dp_sleep_ms(2000);
    }
    dp_err_free(&attempt);
    omt_buf_free(&boot_id);
    if (found) return true;
    if (!dp_cancelled(cancel)) {
        dp_fail(err, "the Raspberry Pi did not come back after reboot within the wait");
    }
    return false;
}

static bool wait_for_appliance(const dp_connection *c, ssh_session *s, const dp_cancel *cancel,
                               const dp_progress *p, dp_err *err) {
    dp_report(p, "Waiting for the OMT appliance to start...");
    omt_buf command, input;
    text_init(&command);
    secret_buf_init(&input);
    dp_privileged_command(c, "docker inspect -f '{{.State.Status}}' omt-client", &command);
    dp_sudo_input(c, &input);
    uint64_t deadline = dp_now_ms() + 3u * 60u * 1000u;
    bool ok = false, failed = false;
    ssh_result r;
    ssh_result_init(&r);
    while (!ok && !failed && dp_now_ms() < deadline) {
        if (!check_cancel(cancel, err) ||
            !run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err)) {
            failed = true;
            break;
        }
        omt_span t = omt_utf8_trim((const char *)r.out.data, r.out.len);
        if (ssh_result_success(&r) && t.len == 7 && memcmp(t.p, "running", 7) == 0) {
            ok = true;
            break;
        }
        dp_sleep_ms(2000);
    }
    if (!ok && !failed) {
        dp_fail(err,
                "the OMT appliance container did not reach running state within three minutes");
    }
    ssh_result_free(&r);
    omt_buf_free(&command);
    omt_buf_free_secret(&input);
    return ok;
}

static bool fetch_initial_web_password(const dp_connection *c, ssh_session *s,
                                       const dp_cancel *cancel, omt_buf *password, dp_err *err) {
    omt_buf command, input, combined;
    text_init(&command);
    secret_buf_init(&input);
    text_init(&combined);
    dp_privileged_argv_command(c, dp_action_argv(DP_ACTION_LOGS), &command);
    dp_sudo_input(c, &input);
    ssh_result r;
    ssh_result_init(&r);
    /* Docker reports the container running as soon as the entrypoint starts,
     * and the entrypoint prints the banner only after it has made the TLS
     * certificate -- seconds later on a Pi 4. So read the logs until either
     * the banner or the server's own start line appears; a redeploy that
     * keeps the stored hash prints no banner at all. */
    uint64_t deadline = dp_now_ms() + 60u * 1000u;
    bool ok = true;
    for (;;) {
        ok = check_cancel(cancel, err) &&
             run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err);
        if (!ok || !ssh_result_success(&r)) break;
        omt_buf_clear(&combined);
        ssh_result_combined(&r, &combined);
        if (dp_first_web_password(omt_buf_cstr(&combined), password)) break;
        if (strstr(omt_buf_cstr(&combined), "omt-web listening on") || dp_now_ms() >= deadline) {
            break;
        }
        ssh_result_free(&r);
        ssh_result_init(&r);
        dp_sleep_ms(2000);
    }
    ssh_result_free(&r);
    omt_buf_free(&command);
    omt_buf_free_secret(&input);
    omt_buf_free_secret(&combined);
    return ok;
}

/* Rebuild the ARM64 image in a developer's working tree. Only reachable with
 * a project root: an operator's deployment uploads the archive compiled into
 * this binary and needs no build tooling at all. */
static bool build_image(const dp_deploy_options *o, const dp_cancel *cancel, const dp_progress *p,
                        dp_err *err) {
    if (!o->rebuild_image) return true;
    if (!o->project_root) {
        dp_fail(err, "rebuilding the appliance image needs a project root to build it from");
        return false;
    }
    dp_report(p, "Building the ARM64 appliance image...");
    /* Windows engines forget their binfmt registration whenever the VM
     * restarts, so it is re-established here; Linux hosts register it
     * persistently through scripts/install-arm64-emulation.sh. */
    if (DP_ON_WINDOWS && !dp_ensure_arm64_emulation(cancel, p, err)) return false;
    dp_build_plan plan;
    if (!dp_image_build_plan(&plan, err)) return false;
    omt_buf summary;
    text_init(&summary);
    dp_build_plan_summary(&plan, &summary);
    dp_reportf(p, "Running %s", omt_buf_cstr(&summary));
    omt_buf_free(&summary);
    dp_process_result result;
    bool ok = dp_run_process(plan.program, (const char *const *)plan.args, o->project_root,
                             (const char *const *)plan.env, cancel, &result, err);
    if (ok && result.exit_code != 0) {
        dp_fail(err, "ARM64 image build failed:\n%s", omt_buf_cstr(&result.output));
        ok = false;
    }
    if (ok || result.output.data) dp_process_result_free(&result);
    dp_build_plan_free(&plan);
    return ok;
}

/* ----------------------------------------------------------- operations */

static bool probe_platform(ssh_session *s, const dp_cancel *cancel, char *board, size_t size,
                           bool *has_board, dp_err *err) {
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, PLATFORM_PROBE, NULL, cancel, &r, err) &&
              dp_require_success(&r, "Remote platform probe", err) &&
              dp_require_supported_appliance(omt_buf_cstr(&r.out), err);
    if (ok) *has_board = dp_probed_board(omt_buf_cstr(&r.out), board, size);
    ssh_result_free(&r);
    return ok;
}

bool dp_test_connection(const dp_connection *c, const dp_cancel *cancel, const dp_progress *p,
                        dp_err *err) {
    if (!check_cancel(cancel, err)) return false;
    dp_report(p, "Testing SSH connection...");
    ssh_session *s;
    if (!connect_checked(c, cancel, &s, err)) return false;
    char board[256];
    bool has_board = false;
    bool ok = probe_platform(s, cancel, board, sizeof(board), &has_board, err);
    ssh_close(s);
    if (!ok) return false;
    if (has_board) {
        dp_reportf(p, "SSH connection succeeded. Detected %s.", board);
    } else {
        dp_report(p, "SSH connection succeeded.");
    }
    return true;
}

/* Install bash and sudo when the target is a stock Alpine image: install.sh
 * and transaction.sh are bash scripts invoked through sudo, so on an untouched
 * Alpine host every later step would fail on a missing interpreter. */
static bool ensure_host_bootstrapped(ssh_session *s, const dp_connection *c,
                                     const char *project_root, const dp_cancel *cancel,
                                     const dp_progress *p, dp_err *err) {
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, TOOLING_PROBE, NULL, cancel, &r, err) &&
              dp_require_success(&r, "Remote tooling probe", err);
    if (!ok) {
        ssh_result_free(&r);
        return false;
    }
    dp_host_tooling tooling = dp_parse_host_tooling(omt_buf_cstr(&r.out));
    if (tooling.has_bash && tooling.has_sudo) {
        ssh_result_free(&r);
        return true;
    }
    dp_report(p, "Bootstrapping bash and sudo on the Raspberry Pi...");
    payload pl;
    omt_buf token, remote, remote_q, command, input, inner;
    text_init(&token);
    text_init(&remote);
    text_init(&remote_q);
    text_init(&command);
    secret_buf_init(&input);
    text_init(&inner);
    ok = capsule_member(project_root, "deploy/host/bootstrap.sh", &pl, err) &&
         dp_random_token(&token, 8, err);
    if (ok) {
        omt_buf_printf(&remote, "/tmp/omt-bootstrap-%s.sh", omt_buf_cstr(&token));
        dp_shell_quote(&remote_q, omt_buf_cstr(&remote));
        ok = upload_payload(s, &pl, omt_buf_cstr(&remote), cancel, err);
    }
    if (ok) {
        /* /bin/sh explicitly: this is the one script that must run before
         * bash does. */
        if (dp_needs_su_bootstrap(&tooling, c)) {
            /* BusyBox su reads from /dev/tty, so a PTY is required. Disable
             * echo before sending the password: channel input may arrive
             * before su has displayed its prompt and must never be reflected
             * into captured logs. */
            omt_buf script;
            text_init(&script);
            omt_buf_printf(&script, "/bin/sh %s", omt_buf_cstr(&remote_q));
            dp_shell_quote(&inner, omt_buf_cstr(&script));
            omt_buf_free(&script);
            omt_buf_printf(&command,
                           "stty -echo; printf '" BOOTSTRAP_PASSWORD_READY "\\n'; su -c %s; "
                           "rc=$?; stty echo; rm -f -- %s; exit $rc",
                           omt_buf_cstr(&inner), omt_buf_cstr(&remote_q));
            omt_buf_append(&input, dp_secret_text(&c->bootstrap_root_password),
                           dp_secret_len(&c->bootstrap_root_password));
            omt_buf_putc(&input, '\n');
            ssh_result_free(&r);
            ssh_result_init(&r);
            ok = ssh_run_pty_after_marker(s, omt_buf_cstr(&command), BOOTSTRAP_PASSWORD_READY,
                                          (const char *)input.data, input.len, cancel, &r, err);
        } else {
            const char *escalation = dp_bootstrap_escalation(&tooling, err);
            ok = escalation != NULL;
            if (ok) {
                omt_buf_printf(&command, "%s /bin/sh %s; rc=$?; rm -f -- %s; exit $rc", escalation,
                               omt_buf_cstr(&remote_q), omt_buf_cstr(&remote_q));
                dp_sudo_input(c, &input);
                ok = run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err);
            }
        }
    }
    ok = ok && dp_require_success(&r, "Alpine bootstrap", err);
    if (pl.path || pl.fingerprint) payload_free(&pl);
    ssh_result_free(&r);
    omt_buf_free(&token);
    omt_buf_free(&remote);
    omt_buf_free(&remote_q);
    omt_buf_free(&command);
    omt_buf_free_secret(&input);
    omt_buf_free(&inner);
    return ok;
}

static bool password_connection(const dp_connection *base, const char *username,
                                const dp_secret *password, const dp_secret *sudo,
                                dp_connection *out) {
    dp_connection_init(out);
    out->host = dp_strdup(base->host);
    out->username = dp_strdup(username);
    out->port = base->port;
    out->auth = DP_AUTH_PASSWORD;
    dp_secret_copy(&out->password, password);
    out->known_hosts_path = base->known_hosts_path ? dp_strdup(base->known_hosts_path) : NULL;
    if (sudo) dp_secret_copy(&out->sudo_password, sudo);
    return out->host && out->username && (!base->known_hosts_path || out->known_hosts_path);
}

static bool reboot_host(ssh_session *s, const dp_connection *c, const omt_buf *input,
                        const char *what, const dp_cancel *cancel, dp_err *err) {
    omt_buf command;
    text_init(&command);
    dp_privileged_argv_command(c, dp_action_argv(DP_ACTION_REBOOT), &command);
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, omt_buf_cstr(&command), input, cancel, &r, err) &&
              dp_require_success(&r, what, err);
    ssh_result_free(&r);
    omt_buf_free(&command);
    return ok;
}

bool dp_alpine_setup(const dp_connection *c, const dp_alpine_settings *settings,
                     const char *project_root, const dp_cancel *cancel, const dp_progress *p,
                     dp_err *err) {
    const char *problem = dp_validate_connection(c);
    if (!problem) problem = dp_validate_alpine_setup(settings);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    if (!check_cancel(cancel, err)) return false;
    dp_report(p, "Connecting and checking the Raspberry Pi...");
    ssh_session *s;
    if (!connect_checked(c, cancel, &s, err)) return false;
    char board[256];
    bool has_board = false;
    payload pl = {0};
    omt_buf token, remote, remote_q, ssid_hex, stdin_text, command, combined, detail, boot_id,
        input;
    text_init(&token);
    text_init(&remote);
    text_init(&remote_q);
    text_init(&ssid_hex);
    secret_buf_init(&stdin_text);
    text_init(&command);
    omt_buf_init(&combined, DP_ERR_LIMIT);
    text_init(&detail);
    text_init(&boot_id);
    secret_buf_init(&input);
    dp_secret psk;
    dp_secret_init(&psk);
    ssh_result r;
    ssh_result_init(&r);
    bool ok = probe_platform(s, cancel, board, sizeof(board), &has_board, err);
    if (ok && has_board) dp_reportf(p, "Installing Alpine sys mode on %s.", board);
    ok = ok && capsule_member(project_root, SETUP_SYS_MEMBER, &pl, err) &&
         dp_random_token(&token, 8, err);
    if (ok) {
        omt_buf_printf(&remote, "/tmp/omt-setup-sys-%s.sh", omt_buf_cstr(&token));
        dp_shell_quote(&remote_q, omt_buf_cstr(&remote));
        dp_report(p, "Uploading the Alpine sys-setup script...");
        ok = upload_payload(s, &pl, omt_buf_cstr(&remote), cancel, err);
    }
    if (ok && settings->has_wifi) {
        dp_hex_encode(&ssid_hex, settings->wifi.ssid, strlen(settings->wifi.ssid));
        const char *pw = dp_secret_text(&settings->wifi.password);
        size_t pw_len = dp_secret_len(&settings->wifi.password);
        bool hex = pw_len == 64;
        for (size_t i = 0; hex && i < 64; i++) {
            char ch = pw[i];
            hex = (ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F');
        }
        if (hex) {
            char lowered[65];
            for (size_t i = 0; i < 64; i++)
                lowered[i] = (char)(pw[i] >= 'A' && pw[i] <= 'F' ? pw[i] + 32 : pw[i]);
            lowered[64] = 0;
            problem = dp_secret_assign(&psk, lowered, 64);
            omt_cleanse(lowered, sizeof(lowered));
        } else {
            problem = dp_derive_wpa_psk(settings->wifi.ssid, &settings->wifi.password, &psk);
        }
        if (problem) {
            dp_fail(err, "%s", problem);
            ok = false;
        }
    }
    if (ok) {
        omt_buf_puts(&stdin_text, settings->hostname);
        omt_buf_putc(&stdin_text, '\n');
        omt_buf_puts(&stdin_text, dp_secret_text(&settings->root_password));
        omt_buf_putc(&stdin_text, '\n');
        omt_buf_puts(&stdin_text, dp_secret_text(&settings->pi_password));
        omt_buf_putc(&stdin_text, '\n');
        omt_buf_append(&stdin_text, ssid_hex.data, ssid_hex.len);
        omt_buf_putc(&stdin_text, '\n');
        omt_buf_puts(&stdin_text, dp_secret_text(&psk));
        omt_buf_putc(&stdin_text, '\n');
        dp_report(p, "Running hostname, DHCP, user, and sys-mode install...");
        omt_buf_printf(&command, "/bin/sh %s; rc=$?; rm -f -- %s; exit $rc",
                       omt_buf_cstr(&remote_q), omt_buf_cstr(&remote_q));
        ok = run_simple(s, omt_buf_cstr(&command), &stdin_text, cancel, &r, err);
    }
    if (ok) {
        const char *secrets[] = {
            dp_secret_text(&settings->root_password),
            dp_secret_text(&settings->pi_password),
            settings->has_wifi ? dp_secret_text(&settings->wifi.password) : "",
            dp_secret_text(&psk),
        };
        ssh_result_combined(&r, &combined);
        if (!ssh_result_success(&r)) {
            dp_redact(omt_buf_cstr(&combined), secrets, OMT_ARRAY_LEN(secrets), &detail);
            fail_with_detail(err, "Alpine sys install failed", omt_buf_cstr(&detail));
            ok = false;
        } else if (!strstr(omt_buf_cstr(&combined), SETUP_SYS_COMPLETE)) {
            dp_fail(err, "Alpine sys install finished without the completion marker");
            ok = false;
        }
    }
    if (ok) {
        dp_report(p, "Alpine sys install finished. Rebooting into the persistent root...");
        dp_sudo_input(c, &input);
        ok = read_boot_id(s, cancel, &boot_id, err) &&
             reboot_host(s, c, &input, "Post-sys-install reboot", cancel, err);
    }
    ssh_close(s);
    s = NULL;
    if (ok) {
        dp_connection pi, root;
        bool built =
            password_connection(c, "pi", &settings->pi_password, &settings->pi_password, &pi) &&
            password_connection(c, "root", &settings->root_password, NULL, &root);
        const dp_connection *both[] = {&pi, &root};
        ssh_session *back = NULL;
        ok = built && wait_for_reboot(both, 2, omt_buf_cstr(&boot_id), 8u * 60u * 1000u, cancel, p,
                                      &back, err);
        if (!built) dp_fail(err, "out of memory");
        ssh_close(back);
        dp_connection_free(&pi);
        dp_connection_free(&root);
    }
    if (ok) {
        dp_report(p, "Persistent sys mode is running. Connect as pi with the password you set, "
                     "then Deploy. The pi account is in wheel; the first deploy installs sudo.");
    }
    payload_free(&pl);
    ssh_result_free(&r);
    dp_secret_clear(&psk);
    omt_buf_free(&token);
    omt_buf_free(&remote);
    omt_buf_free(&remote_q);
    omt_buf_free(&ssid_hex);
    omt_buf_free_secret(&stdin_text);
    omt_buf_free(&command);
    omt_buf_free_secret(&combined);
    omt_buf_free(&detail);
    omt_buf_free(&boot_id);
    omt_buf_free_secret(&input);
    return ok;
}

bool dp_manage(const dp_connection *c, dp_action action, const dp_cancel *cancel,
               const dp_progress *p, dp_err *err) {
    if (!check_cancel(cancel, err)) return false;
    static const char *const TEXT[] = {
        "Fetching container status...",
        "Fetching recent logs...",
        "Restarting service...",
        "Scheduling operating-system reboot...",
    };
    dp_report(p, TEXT[action]);
    ssh_session *s;
    if (!connect_checked(c, cancel, &s, err)) return false;
    omt_buf command, input, output;
    text_init(&command);
    secret_buf_init(&input);
    text_init(&output);
    dp_privileged_argv_command(c, dp_action_argv(action), &command);
    dp_sudo_input(c, &input);
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err) &&
              dp_require_success(&r, "Remote management action", err);
    if (ok) {
        ssh_result_combined(&r, &output);
        omt_span t = omt_utf8_trim((const char *)output.data, output.len);
        if (t.len) dp_report(p, omt_buf_cstr(&output));
    }
    ssh_close(s);
    ssh_result_free(&r);
    omt_buf_free(&command);
    omt_buf_free_secret(&input);
    omt_buf_free(&output);
    return ok;
}

static void connection_secrets(const dp_connection *c, const char **out) {
    out[0] = dp_secret_text(&c->password);
    out[1] = dp_secret_text(&c->key_passphrase);
    out[2] = dp_secret_text(&c->sudo_password);
}

bool dp_set_hostname(const dp_connection *c, const char *hostname, const char *project_root,
                     const dp_cancel *cancel, const dp_progress *p, dp_err *err) {
    const char *problem = dp_validate_connection(c);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    if (!dp_valid_appliance_hostname(hostname)) {
        dp_fail(err, "Hostname must be 1-63 characters: letters, digits, and inner hyphens.");
        return false;
    }
    if (!check_cancel(cancel, err)) return false;
    dp_report(p, "Connecting and uploading the hostname script...");
    ssh_session *s;
    if (!connect_checked(c, cancel, &s, err)) return false;
    payload pl = {0};
    omt_buf token, remote, remote_q, inner, quoted, command, input, combined, detail, summary;
    text_init(&token);
    text_init(&remote);
    text_init(&remote_q);
    text_init(&inner);
    text_init(&quoted);
    text_init(&command);
    secret_buf_init(&input);
    omt_buf_init(&combined, DP_ERR_LIMIT);
    text_init(&detail);
    text_init(&summary);
    ssh_result r;
    ssh_result_init(&r);
    bool ok = capsule_member(project_root, SET_HOSTNAME_MEMBER, &pl, err) &&
              dp_random_token(&token, 8, err);
    if (ok) {
        omt_buf_printf(&remote, "/tmp/omt-set-hostname-%s.sh", omt_buf_cstr(&token));
        dp_shell_quote(&remote_q, omt_buf_cstr(&remote));
        ok = upload_payload(s, &pl, omt_buf_cstr(&remote), cancel, err);
    }
    if (ok) {
        dp_reportf(p, "Renaming the appliance to %s...", hostname);
        /* One privileged process, as with the Wi-Fi and Web-password channels:
         * sudo consumes the first stdin line and the script reads the name
         * from what remains, so the name never appears in a command line or a
         * process listing on the Pi. */
        dp_ssh_rename_command(omt_buf_cstr(&remote_q), &inner);
        omt_buf_puts(&quoted, "sh -c ");
        dp_shell_quote(&quoted, omt_buf_cstr(&inner));
        dp_privileged_stdin_command(c, omt_buf_cstr(&quoted), &command);
        dp_sudo_input(c, &input);
        omt_buf_puts(&input, hostname);
        omt_buf_putc(&input, '\n');
        ok = run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err);
    }
    const char *secrets[3];
    connection_secrets(c, secrets);
    if (ok && !ssh_result_success(&r)) {
        ssh_result_combined(&r, &combined);
        dp_redact(omt_buf_cstr(&combined), secrets, 3, &detail);
        fail_with_detail(err, "Hostname change failed", omt_buf_cstr(&detail));
        ok = false;
    }
    if (ok) {
        /* The script's summary is on stdout. Compose narrates the container's
         * recreation on stderr, which would bury the closing lines. */
        dp_redact(omt_buf_cstr(&r.out), secrets, 3, &summary);
        const char *at = strstr(omt_buf_cstr(&summary), SET_HOSTNAME_COMPLETE);
        if (!at) {
            dp_fail(err, "Hostname change finished without the completion marker");
            ok = false;
        } else {
            const char *tail = at + strlen(SET_HOSTNAME_COMPLETE);
            const char *next = strstr(tail, SET_HOSTNAME_COMPLETE);
            size_t len = next ? (size_t)(next - tail) : strlen(tail);
            omt_span t = omt_utf8_trim(tail, len);
            if (t.len) {
                omt_buf piece;
                text_init(&piece);
                omt_buf_append(&piece, t.p, t.len);
                dp_report(p, omt_buf_cstr(&piece));
                omt_buf_free(&piece);
            }
            dp_reportf(p, "Appliance hostname is now %s.", hostname);
        }
    }
    ssh_close(s);
    payload_free(&pl);
    ssh_result_free(&r);
    omt_buf_free(&token);
    omt_buf_free(&remote);
    omt_buf_free(&remote_q);
    omt_buf_free(&inner);
    omt_buf_free(&quoted);
    omt_buf_free(&command);
    omt_buf_free_secret(&input);
    omt_buf_free_secret(&combined);
    omt_buf_free(&detail);
    omt_buf_free(&summary);
    return ok;
}

void dp_ssh_rename_command(const char *remote_q, omt_buf *out) {
    omt_buf_printf(out, "/bin/sh %s; rc=$?; rm -f -- %s; exit $rc", remote_q, remote_q);
}

bool dp_change_web_password(const dp_connection *c, const dp_secret *password,
                            const dp_cancel *cancel, const dp_progress *p, dp_err *err) {
    const char *problem = dp_validate_connection(c);
    if (!problem) problem = dp_validate_web_password(password);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    if (!check_cancel(cancel, err)) return false;
    dp_report(p, "Changing Web GUI password and restarting the service...");
    ssh_session *s;
    if (!connect_checked(c, cancel, &s, err)) return false;
    omt_buf command, input, combined, detail;
    text_init(&command);
    secret_buf_init(&input);
    omt_buf_init(&combined, DP_ERR_LIMIT);
    text_init(&detail);
    dp_privileged_stdin_command(c, DP_WEB_PASSWORD_COMMAND, &command);
    dp_sudo_input(c, &input);
    omt_buf_append(&input, dp_secret_text(password), dp_secret_len(password));
    omt_buf_putc(&input, '\n');
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err);
    if (ok && !ssh_result_success(&r)) {
        const char *secrets[] = {dp_secret_text(password)};
        ssh_result_combined(&r, &combined);
        dp_redact(omt_buf_cstr(&combined), secrets, 1, &detail);
        fail_with_detail(err, "Web GUI password change failed", omt_buf_cstr(&detail));
        ok = false;
    }
    if (ok) dp_report(p, "Web GUI password changed. Existing Web sessions were revoked.");
    ssh_close(s);
    ssh_result_free(&r);
    omt_buf_free(&command);
    omt_buf_free_secret(&input);
    omt_buf_free_secret(&combined);
    omt_buf_free(&detail);
    return ok;
}

bool dp_apply_wifi(const dp_connection *c, const dp_wifi_settings *settings,
                   const dp_cancel *cancel, const dp_progress *p, dp_err *err) {
    const char *problem = dp_validate_connection(c);
    if (!problem) problem = dp_validate_wifi(settings);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    if (!check_cancel(cancel, err)) return false;
    dp_report(p, settings->connect ? "Applying Wi-Fi settings and requesting a connection..."
                                   : "Saving Wi-Fi profile without connecting...");
    dp_report(p, settings->connect ? "SSH may disconnect if the Raspberry Pi switches networks."
                                   : "Immediate Wi-Fi association is paused; Wi-Fi SSH may "
                                     "disconnect.");
    if (!settings->preserve_existing_profiles) {
        dp_report(p, settings->connect
                         ? "Existing Wi-Fi profiles will be removed after the new profile "
                           "associates."
                         : "Existing Wi-Fi profiles will be removed without testing the new "
                           "profile; Wi-Fi SSH may disconnect.");
    }
    const char *password = dp_secret_text(&settings->password);
    size_t password_len = dp_secret_len(&settings->password);
    dp_secret psk;
    dp_secret_init(&psk);
    bool hex = password_len == 64;
    for (size_t i = 0; hex && i < 64; i++) {
        char ch = password[i];
        hex = (ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F');
    }
    if (hex) {
        char lowered[65];
        for (size_t i = 0; i < 64; i++) {
            lowered[i] =
                (char)(password[i] >= 'A' && password[i] <= 'F' ? password[i] + 32 : password[i]);
        }
        lowered[64] = 0;
        problem = dp_secret_assign(&psk, lowered, 64);
        omt_cleanse(lowered, sizeof(lowered));
    } else {
        problem = dp_derive_wpa_psk(settings->ssid, &settings->password, &psk);
    }
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    omt_buf token, marker, ssid_hex, input, wifi_command, command, combined, detail;
    text_init(&token);
    text_init(&marker);
    text_init(&ssid_hex);
    secret_buf_init(&input);
    text_init(&wifi_command);
    text_init(&command);
    omt_buf_init(&combined, DP_ERR_LIMIT);
    text_init(&detail);
    bool ok = dp_random_token(&token, 12, err);
    ssh_session *s = NULL;
    ssh_result r;
    ssh_result_init(&r);
    if (ok) {
        omt_buf_printf(&marker, "__OMT_WIFI_PASSWORD_FOLLOWS_%s__", omt_buf_cstr(&token));
        dp_hex_encode(&ssid_hex, settings->ssid, strlen(settings->ssid));
        dp_sudo_input(c, &input);
        omt_buf_append(&input, marker.data, marker.len);
        omt_buf_putc(&input, '\n');
        omt_buf_append(&input, psk.value.data, psk.value.len);
        omt_buf_putc(&input, '\n');
        omt_buf_puts(&wifi_command, "sh -eu -c ");
        dp_shell_quote(&wifi_command, DP_WIFI_SCRIPT);
        omt_buf_puts(&wifi_command, " sh ");
        dp_shell_quote(&wifi_command, omt_buf_cstr(&ssid_hex));
        omt_buf_putc(&wifi_command, ' ');
        dp_shell_quote(&wifi_command, settings->connect ? "yes" : "no");
        omt_buf_putc(&wifi_command, ' ');
        dp_shell_quote(&wifi_command, settings->preserve_existing_profiles ? "yes" : "no");
        omt_buf_putc(&wifi_command, ' ');
        dp_shell_quote(&wifi_command, omt_buf_cstr(&marker));
        omt_buf_putc(&wifi_command, ' ');
        dp_shell_quote(&wifi_command, settings->ssid);
        /* Authenticate and execute in one sudo process. Alpine's default sudo
         * timestamp policy may not carry a non-interactive `sudo -v` ticket
         * into a second `sudo -n` invocation, even in the same SSH channel.
         * The first input line is consumed by sudo; the marker and PSK remain
         * available to the privileged shell on the same stdin stream. */
        dp_privileged_stdin_command(c, omt_buf_cstr(&wifi_command), &command);
        ok = connect_checked(c, cancel, &s, err) &&
             run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err);
    }
    if (ok && !ssh_result_success(&r)) {
        const char *secrets[5];
        connection_secrets(c, secrets);
        secrets[3] = dp_secret_text(&psk);
        secrets[4] = password;
        ssh_result_combined(&r, &combined);
        dp_redact(omt_buf_cstr(&combined), secrets, 5, &detail);
        dp_fail(err, "Wi-Fi update failed:\n%s", omt_buf_cstr(&detail));
        ok = false;
    }
    if (ok)
        dp_report(p, settings->connect ? "Wi-Fi settings applied and connection requested."
                                       : "Wi-Fi settings saved.");
    ssh_close(s);
    ssh_result_free(&r);
    dp_secret_clear(&psk);
    omt_buf_free(&token);
    omt_buf_free(&marker);
    omt_buf_free(&ssid_hex);
    omt_buf_free_secret(&input);
    omt_buf_free(&wifi_command);
    omt_buf_free(&command);
    omt_buf_free_secret(&combined);
    omt_buf_free(&detail);
    return ok;
}

/* ------------------------------------------------------------- deploy */

typedef struct {
    const char *stage;
    const char *token;
    const char *remote_q;
} staging;

/* Deliberately uncancellable: this is the failure path, and the operator
 * cancelling is one of the reasons it runs. */
static void remove_stage(ssh_session *s, const char *stage_q) {
    omt_buf command;
    text_init(&command);
    omt_buf_printf(&command,
                   "if [ -d %s ] && [ ! -L %s ]; then find -P %s -xdev -depth -delete; fi", stage_q,
                   stage_q, stage_q);
    ssh_result r;
    ssh_result_init(&r);
    dp_err ignored;
    dp_err_init(&ignored);
    (void)run_simple(s, omt_buf_cstr(&command), NULL, NULL, &r, &ignored);
    dp_err_free(&ignored);
    ssh_result_free(&r);
    omt_buf_free(&command);
}

static bool run_required(ssh_session *s, const char *command, const omt_buf *input,
                         const char *what, const dp_cancel *cancel, dp_err *err) {
    ssh_result r;
    ssh_result_init(&r);
    bool ok = run_simple(s, command, input, cancel, &r, err) && dp_require_success(&r, what, err);
    ssh_result_free(&r);
    return ok;
}

/* Uploads every manifest member, verifies each one's remote digest, and
 * promotes the set. The caller removes the staging directory if this fails. */
static bool stage_and_promote(ssh_session *s, const artifact *a, size_t n, const staging *st,
                              const dp_cancel *cancel, const dp_progress *p, dp_err *err) {
    omt_buf command, remote_path, quoted, parent;
    text_init(&command);
    text_init(&remote_path);
    text_init(&quoted);
    text_init(&parent);
    bool ok = true;
    for (size_t i = 0; ok && i < n; i++) {
        const char *name = a[i].name;
        if (!check_cancel(cancel, err)) {
            ok = false;
            break;
        }
        const char *slash = strrchr(name, '/');
        if (slash && slash != name) {
            omt_buf_clear(&parent);
            omt_buf_printf(&parent, "%s/%.*s", st->stage, (int)(slash - name), name);
            omt_buf_clear(&command);
            omt_buf_puts(&command, "mkdir -p -- ");
            dp_shell_quote(&command, omt_buf_cstr(&parent));
            ok = run_required(s, omt_buf_cstr(&command), NULL, "Remote staging preparation", cancel,
                              err);
            if (!ok) break;
        }
        dp_reportf(p, "Uploading %s...", name);
        omt_buf_clear(&remote_path);
        omt_buf_printf(&remote_path, "%s/%s", st->stage, name);
        ok = upload_payload(s, &a[i].data, omt_buf_cstr(&remote_path), cancel, err);
        bool unchanged = true;
        ok = ok && artifact_unchanged(&a[i], &unchanged, err);
        if (ok && !unchanged) {
            dp_fail(err, "local deployment artifact changed during upload: %s", name);
            ok = false;
        }
        if (!ok) break;
        omt_buf_clear(&command);
        omt_buf_puts(&command, "sha256sum -- ");
        dp_shell_quote(&command, omt_buf_cstr(&remote_path));
        ssh_result r;
        ssh_result_init(&r);
        ok = run_simple(s, omt_buf_cstr(&command), NULL, cancel, &r, err) &&
             dp_require_success(&r, "Remote checksum", err);
        char digest[65];
        if (ok && (!dp_parse_sha256_line(omt_buf_cstr(&r.out), digest) ||
                   strcmp(digest, a[i].digest) != 0)) {
            dp_fail(err, "SHA-256 mismatch after uploading %s", name);
            ok = false;
        }
        ssh_result_free(&r);
        if (ok) dp_reportf(p, "Verified SHA-256 for %s.", name);
    }
    if (ok) {
        omt_buf_clear(&command);
        omt_buf_puts(&command, "bash ");
        omt_buf_clear(&quoted);
        omt_buf_printf(&quoted, "%s/deploy/transaction.sh", st->stage);
        dp_shell_quote(&command, omt_buf_cstr(&quoted));
        omt_buf_printf(&command, " promote %s ", st->remote_q);
        dp_shell_quote(&command, st->token);
        omt_buf_putc(&command, ' ');
        omt_buf_clear(&quoted);
        omt_buf_printf(&quoted, "%s/deploy/manifest-v3.txt", st->stage);
        dp_shell_quote(&command, omt_buf_cstr(&quoted));
        ok = run_required(s, omt_buf_cstr(&command), NULL, "Deployment promotion", cancel, err);
    }
    omt_buf_free(&command);
    omt_buf_free(&remote_path);
    omt_buf_free(&quoted);
    omt_buf_free(&parent);
    return ok;
}

static const char *const EXECUTABLE_PATHS[] = {
    "deploy/host/bootstrap.sh",
    "deploy/host/setup-sys.sh",
    "deploy/host/set-hostname.sh",
    "deploy/host/install.sh",
    "deploy/host/uninstall.sh",
    "deploy/host/host-diagnostics.sh",
    "deploy/host/host-event-watcher.sh",
    "deploy/host/host-reboot.sh",
    "deploy/transaction.sh",
};

static void quote_join(omt_buf *out, const char *a, const char *b) {
    omt_buf joined;
    text_init(&joined);
    omt_buf_printf(&joined, "%s%s", a, b);
    dp_shell_quote(out, omt_buf_cstr(&joined));
    omt_buf_free(&joined);
}

bool dp_deploy(const dp_connection *c, const dp_deploy_options *o, const dp_cancel *cancel,
               const dp_progress *p, dp_err *err) {
    const char *problem = dp_validate_connection(c);
    if (!problem) problem = dp_validate_options(o);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    if (!check_cancel(cancel, err) || !build_image(o, cancel, p, err) ||
        !check_cancel(cancel, err)) {
        return false;
    }
    artifact *arts = NULL;
    size_t count = 0;
    if (!capsule_artifacts(o, &arts, &count, err)) return false;
    omt_buf text;
    text_init(&text);
    capsule_summary(arts, count, o, &text);
    dp_report(p, omt_buf_cstr(&text));
    omt_buf_clear(&text);

    dp_report(p, "Connecting and checking the Raspberry Pi...");
    ssh_session *s = NULL;
    char board[256];
    bool has_board = false;
    bool ok = connect_checked(c, cancel, &s, err) &&
              probe_platform(s, cancel, board, sizeof(board), &has_board, err);
    if (ok && has_board) dp_reportf(p, "Deploying to %s.", board);
    ok = ok && ensure_host_bootstrapped(s, c, o->project_root, cancel, p, err);

    /* The remote directory without trailing slashes. */
    char *remote_directory = dp_strdup(o->remote_directory);
    if (remote_directory) {
        size_t len = strlen(remote_directory);
        while (len > 0 && remote_directory[len - 1] == '/') remote_directory[--len] = 0;
    }
    omt_buf remote_q, input, command, token, staging_root, stage, staging_q, stage_q, summary,
        boot_id, password;
    text_init(&remote_q);
    secret_buf_init(&input);
    text_init(&command);
    text_init(&token);
    text_init(&staging_root);
    text_init(&stage);
    text_init(&staging_q);
    text_init(&stage_q);
    text_init(&summary);
    text_init(&boot_id);
    secret_buf_init(&password);
    if (ok && !remote_directory) {
        dp_fail(err, "out of memory");
        ok = false;
    }
    if (ok) {
        dp_shell_quote(&remote_q, remote_directory);
        dp_sudo_input(c, &input);
        omt_buf_printf(&text, "install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" %s",
                       omt_buf_cstr(&remote_q));
        dp_privileged_command(c, omt_buf_cstr(&text), &command);
        ok = run_required(s, omt_buf_cstr(&command), &input, "Remote directory preparation", cancel,
                          err);
    }
    ok = ok && dp_random_token(&token, 12, err);
    if (ok) {
        omt_buf_printf(&staging_root, "%s/.deploy-staging", remote_directory);
        omt_buf_printf(&stage, "%s/%s", omt_buf_cstr(&staging_root), omt_buf_cstr(&token));
        dp_shell_quote(&staging_q, omt_buf_cstr(&staging_root));
        dp_shell_quote(&stage_q, omt_buf_cstr(&stage));

        omt_buf legacy, legacy_manifest, current;
        text_init(&legacy);
        text_init(&legacy_manifest);
        text_init(&current);
        quote_join(&legacy, remote_directory, "/deploy-transaction.sh");
        quote_join(&legacy_manifest, remote_directory, "/deploy-artifacts.txt");
        quote_join(&current, remote_directory, "/deploy/transaction.sh");
        omt_buf_clear(&command);
        omt_buf_printf(&command,
                       "if [ -x %s ] && [ -f %s ]; then %s recover %s %s; fi; if [ -x %s ]; then "
                       "%s recover %s; fi",
                       omt_buf_cstr(&legacy), omt_buf_cstr(&legacy_manifest), omt_buf_cstr(&legacy),
                       omt_buf_cstr(&remote_q), omt_buf_cstr(&legacy_manifest),
                       omt_buf_cstr(&current), omt_buf_cstr(&current), omt_buf_cstr(&remote_q));
        omt_buf_free(&legacy);
        omt_buf_free(&legacy_manifest);
        omt_buf_free(&current);
        ok = run_required(s, omt_buf_cstr(&command), NULL, "Interrupted deployment recovery",
                          cancel, err);
    }
    if (ok) {
        omt_buf_clear(&command);
        omt_buf_printf(&command,
                       "if [ -L %s ] || { [ -e %s ] && [ ! -d %s ]; }; then exit 14; fi; "
                       "install -d -m 700 -- %s; mkdir -- %s",
                       omt_buf_cstr(&staging_q), omt_buf_cstr(&staging_q), omt_buf_cstr(&staging_q),
                       omt_buf_cstr(&staging_q), omt_buf_cstr(&stage_q));
        ok = run_required(s, omt_buf_cstr(&command), NULL, "Remote staging root validation", cancel,
                          err);
    }
    if (ok) {
        /* One owner for the staging directory: every failure between here and
         * a completed promotion removes it, a transport error included. */
        staging st = {omt_buf_cstr(&stage), omt_buf_cstr(&token), omt_buf_cstr(&remote_q)};
        ok = stage_and_promote(s, arts, count, &st, cancel, p, err);
        if (!ok) remove_stage(s, omt_buf_cstr(&stage_q));
    }
    if (ok) {
        omt_buf chmod, installer, install_script, inner;
        text_init(&chmod);
        text_init(&installer);
        text_init(&install_script);
        text_init(&inner);
        omt_buf_puts(&chmod, "chmod +x");
        for (size_t i = 0; i < OMT_ARRAY_LEN(EXECUTABLE_PATHS); i++) {
            omt_buf_putc(&chmod, ' ');
            omt_buf_clear(&text);
            omt_buf_printf(&text, "/%s", EXECUTABLE_PATHS[i]);
            quote_join(&chmod, remote_directory, omt_buf_cstr(&text));
        }
        quote_join(&installer, remote_directory, "/deploy/host/install.sh");
        omt_buf_clear(&text);
        omt_buf_printf(&text, "printf 'n\\n' | %s", omt_buf_cstr(&installer));
        dp_shell_quote(&install_script, omt_buf_cstr(&text));
        omt_buf_clear(&text);
        omt_buf_printf(&text, "sh -c %s", omt_buf_cstr(&install_script));
        dp_privileged_command(c, omt_buf_cstr(&text), &inner);
        omt_buf_clear(&command);
        omt_buf_printf(&command, "%s && %s", omt_buf_cstr(&chmod), omt_buf_cstr(&inner));
        ssh_result r;
        ssh_result_init(&r);
        ok = run_simple(s, omt_buf_cstr(&command), &input, cancel, &r, err) &&
             dp_require_success(&r, "Remote installer", err);
        /* The installer prints its operator summary on stdout. OpenRC and
         * Compose warnings arrive on stderr and are deliberately excluded. */
        if (ok && dp_installer_summary(omt_buf_cstr(&r.out), &summary)) {
            const char *secrets[4];
            connection_secrets(c, secrets);
            secrets[3] = dp_secret_text(&c->bootstrap_root_password);
            omt_buf redacted;
            text_init(&redacted);
            dp_redact(omt_buf_cstr(&summary), secrets, 4, &redacted);
            dp_report(p, omt_buf_cstr(&redacted));
            omt_buf_free(&redacted);
        }
        ssh_result_free(&r);
        omt_buf_free(&chmod);
        omt_buf_free(&installer);
        omt_buf_free(&install_script);
        omt_buf_free(&inner);
    }
    if (ok) {
        dp_report(p, "Rebooting to apply kernel, firmware, and KMS settings...");
        ok = read_boot_id(s, cancel, &boot_id, err) &&
             reboot_host(s, c, &input, "Post-install reboot", cancel, err);
    }
    ssh_close(s);
    s = NULL;
    if (ok) {
        const dp_connection *only[] = {c};
        ok = wait_for_reboot(only, 1, omt_buf_cstr(&boot_id), 6u * 60u * 1000u, cancel, p, &s, err);
    }
    if (ok) {
        dp_err wait_error;
        dp_err_init(&wait_error);
        if (wait_for_appliance(c, s, cancel, p, &wait_error)) {
            ok = fetch_initial_web_password(c, s, cancel, &password, err);
            if (ok && password.len) {
                dp_reportf(p, "Web UI password (save this now): %s", omt_buf_cstr(&password));
            }
            if (ok) dp_report(p, "Appliance is running.");
        } else {
            dp_reportf(p,
                       "Install succeeded, but the appliance did not start within the wait: %s. "
                       "Check `sudo rc-service omt-client status` on the Pi.",
                       dp_err_text(&wait_error));
        }
        dp_err_free(&wait_error);
    }
    if (ok) dp_report(p, "Deployment complete.");
    ssh_close(s);
    free(remote_directory);
    artifacts_free(arts, count);
    omt_buf_free(&text);
    omt_buf_free(&remote_q);
    omt_buf_free_secret(&input);
    omt_buf_free(&command);
    omt_buf_free(&token);
    omt_buf_free(&staging_root);
    omt_buf_free(&stage);
    omt_buf_free(&staging_q);
    omt_buf_free(&stage_q);
    omt_buf_free(&summary);
    omt_buf_free(&boot_id);
    omt_buf_free_secret(&password);
    return ok;
}
