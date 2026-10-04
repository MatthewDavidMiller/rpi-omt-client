/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Secrets, validation, and the small helpers the operations share.
 */
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "common/rand.h"
#include "crypto/crypto.h"
#include "deploy/core/deploy.h"

/* ----------------------------------------------------------- string utils */

char *dp_strndup(const char *s, size_t len) {
    char *copy = malloc(len + 1);
    if (!copy) return NULL;
    memcpy(copy, s, len);
    copy[len] = 0;
    return copy;
}

char *dp_strdup(const char *s) { return s ? dp_strndup(s, strlen(s)) : NULL; }

char *dp_trim_dup(const char *s) {
    if (!s) return dp_strdup("");
    omt_span t = omt_utf8_trim(s, strlen(s));
    return dp_strndup((const char *)t.p, t.len);
}

size_t dp_utf8_count(const char *text, size_t len) {
    size_t count = 0;
    for (size_t i = 0; i < len; i++) {
        if (((uint8_t)text[i] & 0xC0) != 0x80) count++;
    }
    return count;
}

bool dp_has_control(const char *text, size_t len) {
    if (!omt_utf8_valid(text, len)) return true;
    size_t i = 0;
    while (i < len) {
        uint32_t cp = omt_utf8_next(text, len, &i);
        if (cp < 0x20 || (cp >= 0x7F && cp <= 0x9F)) return true;
    }
    return false;
}

/* ---------------------------------------------------------------- secrets */

void dp_secret_init(dp_secret *s) {
    s->set = false;
    omt_buf_init(&s->value, DP_MAX_SECRET_BYTES + 1);
}

void dp_secret_clear(dp_secret *s) {
    omt_buf_free_secret(&s->value);
    omt_buf_init(&s->value, DP_MAX_SECRET_BYTES + 1);
    s->set = false;
}

const char *dp_secret_assign(dp_secret *s, const char *text, size_t len) {
    dp_secret_clear(s);
    if (len > DP_MAX_SECRET_BYTES || dp_has_control(text, len)) {
        return "Authentication secret is invalid or exceeds 4096 bytes.";
    }
    /* Sized once, so the value is never reallocated and copied into memory
     * that would be freed unwiped. */
    if (!omt_buf_reserve(&s->value, len + 1)) return "Authentication secret could not be stored.";
    omt_buf_append(&s->value, text, len);
    s->set = true;
    return NULL;
}

const char *dp_secret_assign_cstr(dp_secret *s, const char *text) {
    return dp_secret_assign(s, text, strlen(text));
}

void dp_secret_copy(dp_secret *dst, const dp_secret *src) {
    dp_secret_clear(dst);
    if (!src->set) return;
    if (omt_buf_reserve(&dst->value, src->value.len + 1)) {
        omt_buf_append(&dst->value, src->value.data, src->value.len);
    }
    dst->set = true;
}

/* -------------------------------------------------------------- settings */

static const char *const ACTION_STATUS[] = {"docker", "ps", "--filter", "name=omt-client", NULL};
static const char *const ACTION_LOGS[] = {"docker", "logs", "--tail", "500", "omt-client", NULL};
/* Manage the OpenRC service rather than assuming its container already
 * exists: a clean install defers first startup until reboot, and rc-service
 * can both start that state and restart an existing container. */
static const char *const ACTION_RESTART[] = {"rc-service", "omt-client", "restart", NULL};
/* Return a successful SSH exit status before the kernel tears the connection
 * down. Every token is fixed; no form value can select a command or argument
 * across this privilege boundary. */
static const char *const ACTION_REBOOT[] = {
    "sh", "-c", "nohup sh -c 'sleep 1; /sbin/reboot' </dev/null >/dev/null 2>&1 &", NULL};

const char *const *dp_action_argv(dp_action action) {
    switch (action) {
    case DP_ACTION_STATUS: return ACTION_STATUS;
    case DP_ACTION_LOGS: return ACTION_LOGS;
    case DP_ACTION_RESTART: return ACTION_RESTART;
    case DP_ACTION_REBOOT: return ACTION_REBOOT;
    }
    return ACTION_STATUS;
}

void dp_connection_init(dp_connection *c) {
    memset(c, 0, sizeof(*c));
    c->port = 22;
    c->auth = DP_AUTH_PASSWORD;
    dp_secret_init(&c->password);
    dp_secret_init(&c->key_passphrase);
    dp_secret_init(&c->sudo_password);
    dp_secret_init(&c->bootstrap_root_password);
}

void dp_connection_free(dp_connection *c) {
    free(c->host);
    free(c->username);
    free(c->key_path);
    free(c->known_hosts_path);
    c->host = c->username = c->key_path = c->known_hosts_path = NULL;
    dp_secret_clear(&c->password);
    dp_secret_clear(&c->key_passphrase);
    dp_secret_clear(&c->sudo_password);
    dp_secret_clear(&c->bootstrap_root_password);
}

static bool dup_into(char **dst, const char *src) {
    free(*dst);
    *dst = NULL;
    if (!src) return true;
    *dst = dp_strdup(src);
    return *dst != NULL;
}

bool dp_connection_copy(dp_connection *dst, const dp_connection *src) {
    bool ok = dup_into(&dst->host, src->host) && dup_into(&dst->username, src->username) &&
              dup_into(&dst->key_path, src->key_path) &&
              dup_into(&dst->known_hosts_path, src->known_hosts_path);
    dst->port = src->port;
    dst->auth = src->auth;
    dp_secret_copy(&dst->password, &src->password);
    dp_secret_copy(&dst->key_passphrase, &src->key_passphrase);
    dp_secret_copy(&dst->sudo_password, &src->sudo_password);
    dp_secret_copy(&dst->bootstrap_root_password, &src->bootstrap_root_password);
    return ok;
}

void dp_deploy_options_free(dp_deploy_options *o) {
    free(o->project_root);
    free(o->remote_directory);
    o->project_root = o->remote_directory = NULL;
}

void dp_wifi_settings_free(dp_wifi_settings *w) {
    free(w->ssid);
    w->ssid = NULL;
    dp_secret_clear(&w->password);
}

void dp_alpine_settings_free(dp_alpine_settings *a) {
    free(a->hostname);
    a->hostname = NULL;
    if (a->has_wifi) dp_wifi_settings_free(&a->wifi);
    dp_secret_clear(&a->root_password);
    dp_secret_clear(&a->pi_password);
}

void dp_sd_settings_free(dp_sd_settings *s) {
    free(s->boot_directory);
    free(s->country);
    free(s->wifi_ssid);
    s->boot_directory = s->country = s->wifi_ssid = NULL;
    dp_secret_clear(&s->wifi_password);
}

/* ------------------------------------------------------------ validation */

static bool alnum(uint8_t b) {
    return (b >= '0' && b <= '9') || (b >= 'a' && b <= 'z') || (b >= 'A' && b <= 'Z');
}

static bool ascii_token(const char *value, size_t len, const char *extra) {
    if (len == 0) return false;
    for (size_t i = 0; i < len; i++) {
        uint8_t b = (uint8_t)value[i];
        if (!alnum(b) && !(b && strchr(extra, b))) return false;
    }
    return true;
}

bool dp_valid_host(const char *value) {
    size_t len = strlen(value);
    if (len == 0 || len > 253) return false;
    const char *label = value;
    for (;;) {
        const char *dot = strchr(label, '.');
        size_t n = dot ? (size_t)(dot - label) : strlen(label);
        if (n == 0 || n > 63 || label[0] == '-' || label[n - 1] == '-' ||
            !ascii_token(label, n, "-")) {
            return false;
        }
        if (!dot) return true;
        label = dot + 1;
    }
}

bool dp_valid_username(const char *value) {
    size_t len = strlen(value);
    return len <= 64 && ascii_token(value, len, "._-");
}

bool dp_valid_appliance_hostname(const char *value) {
    size_t len = strlen(value);
    return len >= 1 && len <= 63 && value[0] != '-' && value[len - 1] != '-' &&
           ascii_token(value, len, "-");
}

bool dp_valid_manifest_name(const char *value) {
    size_t len = strlen(value);
    if (len == 0 || len > DP_MAX_MANIFEST_MEMBER_BYTES || value[0] == '/' ||
        value[len - 1] == '/' || strstr(value, "//")) {
        return false;
    }
    for (size_t i = 0; i < len; i++) {
        uint8_t b = (uint8_t)value[i];
        if (!alnum(b) && b != '.' && b != '_' && b != '-' && b != '/') return false;
    }
    const char *part = value;
    for (;;) {
        const char *slash = strchr(part, '/');
        size_t n = slash ? (size_t)(slash - part) : strlen(part);
        if (n == 0 || (n == 1 && part[0] == '.') || (n == 2 && part[0] == '.' && part[1] == '.')) {
            return false;
        }
        if (!slash) return true;
        part = slash + 1;
    }
}

bool dp_valid_remote_directory(const char *value) {
    size_t len = strlen(value);
    return len >= 2 && len <= DP_MAX_MANIFEST_MEMBER_BYTES && value[0] == '/' &&
           value[len - 1] != '/' && dp_valid_manifest_name(value + 1);
}

const char *dp_validate_connection(const dp_connection *c) {
    if (!c->host || !dp_valid_host(c->host)) {
        return "Pi host must be a valid IPv4 address or DNS host name.";
    }
    if (!c->username || !dp_valid_username(c->username)) {
        return "SSH username contains invalid characters.";
    }
    if (c->port == 0) return "SSH port must be between 1 and 65535.";
    if (c->known_hosts_path && !dp_is_file(c->known_hosts_path)) {
        return "OpenSSH known_hosts file does not exist.";
    }
    if (c->auth == DP_AUTH_PASSWORD && !c->password.set) {
        return "SSH password is required for password authentication.";
    }
    if (c->auth == DP_AUTH_KEY && (!c->key_path || !dp_is_file(c->key_path))) {
        return "SSH private-key file does not exist.";
    }
    return NULL;
}

const char *dp_validate_options(const dp_deploy_options *o) {
    if (o->project_root && !dp_is_dir(o->project_root)) return "Project root does not exist.";
    if (!o->project_root && o->rebuild_image) {
        return "Rebuilding the appliance image needs a project root to build it from.";
    }
    if (!o->remote_directory || !dp_valid_remote_directory(o->remote_directory)) {
        return "Remote install directory is not a normalized safe absolute path.";
    }
    return NULL;
}

static bool is_hex_psk(const char *text, size_t len) {
    if (len != 64) return false;
    for (size_t i = 0; i < len; i++) {
        uint8_t b = (uint8_t)text[i];
        if (!((b >= '0' && b <= '9') || (b >= 'a' && b <= 'f') || (b >= 'A' && b <= 'F'))) {
            return false;
        }
    }
    return true;
}

const char *dp_validate_wifi(const dp_wifi_settings *w) {
    size_t ssid_len = w->ssid ? strlen(w->ssid) : 0;
    if (ssid_len == 0 || ssid_len > 32 || dp_has_control(w->ssid, ssid_len)) {
        return "Wi-Fi SSID must contain 1-32 UTF-8 bytes and no control characters.";
    }
    const char *password = dp_secret_text(&w->password);
    size_t len = dp_secret_len(&w->password);
    bool printable = len >= 8 && len <= 63;
    for (size_t i = 0; printable && i < len; i++) {
        uint8_t b = (uint8_t)password[i];
        printable = b >= 0x20 && b <= 0x7e;
    }
    if (!is_hex_psk(password, len) && !printable) {
        return "Wi-Fi password must be 8-63 printable ASCII characters or a 64-digit hex PSK.";
    }
    return NULL;
}

const char *dp_validate_os_password(const dp_secret *s) {
    size_t len = dp_secret_len(s);
    if (len < DP_MIN_OS_PASSWORD_BYTES || len > DP_MAX_OS_PASSWORD_BYTES) {
        return "Host password must contain 8-128 UTF-8 bytes.";
    }
    if (dp_has_control(dp_secret_text(s), len)) {
        return "Host password must not contain control characters.";
    }
    return NULL;
}

const char *dp_validate_alpine_setup(const dp_alpine_settings *a) {
    if (!a->hostname || !dp_valid_appliance_hostname(a->hostname)) {
        return "Hostname must be a single DNS label of 1-63 letters, digits, or hyphens.";
    }
    const char *problem = dp_validate_os_password(&a->root_password);
    if (!problem) problem = dp_validate_os_password(&a->pi_password);
    if (!problem && a->has_wifi) problem = dp_validate_wifi(&a->wifi);
    return problem;
}

const char *dp_validate_web_password(const dp_secret *s) {
    size_t len = dp_secret_len(s);
    if (len < DP_MIN_WEB_PASSWORD_BYTES || len > DP_MAX_WEB_PASSWORD_BYTES) {
        return "Web GUI password must contain 12-128 UTF-8 bytes.";
    }
    if (dp_has_control(dp_secret_text(s), len)) {
        return "Web GUI password must not contain control characters.";
    }
    return NULL;
}

/* --------------------------------------------------------------- helpers */

void dp_hex_encode(omt_buf *out, const void *bytes, size_t len) { omt_buf_hex(out, bytes, len); }

const char *dp_derive_wpa_psk(const char *ssid, const dp_secret *passphrase, dp_secret *out) {
    size_t ssid_len = strlen(ssid);
    size_t len = dp_secret_len(passphrase);
    if (ssid_len == 0 || ssid_len > 32 || len < 8 || len > 63) return "Invalid WPA credentials.";
    uint8_t derived[32];
    if (!omt_pbkdf2_sha1(dp_secret_text(passphrase), len, ssid, ssid_len, 4096, derived,
                         sizeof(derived))) {
        omt_cleanse(derived, sizeof(derived));
        return "WPA derivation failed.";
    }
    static const char digits[] = "0123456789abcdef";
    char encoded[65];
    for (size_t i = 0; i < 32; i++) {
        encoded[2 * i] = digits[derived[i] >> 4];
        encoded[2 * i + 1] = digits[derived[i] & 15];
    }
    encoded[64] = 0;
    omt_cleanse(derived, sizeof(derived));
    const char *problem = dp_secret_assign(out, encoded, 64);
    omt_cleanse(encoded, sizeof(encoded));
    return problem;
}

void dp_shell_quote(omt_buf *out, const char *value) {
    omt_buf_putc(out, '\'');
    for (const char *p = value; *p; p++) {
        if (*p == '\'') {
            omt_buf_puts(out, "'\\''");
        } else {
            omt_buf_putc(out, (uint8_t)*p);
        }
    }
    omt_buf_putc(out, '\'');
}

bool dp_random_token(omt_buf *out, size_t bytes, dp_err *err) {
    if (bytes == 0 || bytes > 64) {
        dp_fail(err, "invalid token size");
        return false;
    }
    omt_err e;
    if (!omt_random_hex(out, bytes, &e)) {
        dp_fail(err, "%s", e.msg);
        return false;
    }
    return true;
}

static void hex64(const uint8_t digest[32], char out[65]) {
    static const char digits[] = "0123456789abcdef";
    for (size_t i = 0; i < 32; i++) {
        out[2 * i] = digits[digest[i] >> 4];
        out[2 * i + 1] = digits[digest[i] & 15];
    }
    out[64] = 0;
}

void dp_sha256_hex(const void *data, size_t len, char out[65]) {
    uint8_t digest[32] = {0};
    if (!omt_sha256(data, len, digest)) memset(digest, 0, sizeof(digest));
    hex64(digest, out);
}

bool dp_sha256_file(const char *path, char out[65], dp_err *err) {
    dp_file *f = dp_file_open_read(path, err);
    if (!f) return false;
    omt_sha256_ctx *ctx = omt_sha256_begin();
    if (!ctx) {
        dp_file_close(f);
        dp_fail(err, "SHA-256 is unavailable");
        return false;
    }
    uint8_t *chunk = malloc(64u * 1024u);
    bool ok = chunk != NULL;
    while (ok) {
        long n = dp_file_read(f, chunk, 64u * 1024u, err);
        if (n < 0) {
            ok = false;
            break;
        }
        if (n == 0) break;
        if (!omt_sha256_update(ctx, chunk, (size_t)n)) ok = false;
    }
    free(chunk);
    dp_file_close(f);
    uint8_t digest[32];
    if (!omt_sha256_finish(ctx, digest) || !ok) {
        if (!err || err->msg.len == 0) dp_fail(err, "cannot hash %s", path);
        return false;
    }
    hex64(digest, out);
    return true;
}

/* --------------------------------------------------------------- manifest */

void dp_manifest_free(dp_manifest *m) {
    for (size_t i = 0; i < m->count; i++) free(m->names[i]);
    free(m->names);
    m->names = NULL;
    m->count = 0;
}

bool dp_parse_manifest(const char *text, size_t len, dp_manifest *out, dp_err *err) {
    out->names = NULL;
    out->count = 0;
    size_t pos = 0;
    bool first = true;
    bool has_transaction = false, has_manifest = false;
    /* Lines as Rust's str::lines splits them: on \n, with one trailing \r
     * removed, and no empty final line after a terminating newline. */
    while (pos < len) {
        const char *line = text + pos;
        const char *nl = memchr(line, '\n', len - pos);
        size_t n = nl ? (size_t)(nl - line) : len - pos;
        pos += n + (nl ? 1 : 0);
        if (n > 0 && line[n - 1] == '\r') n--;
        if (first) {
            first = false;
            if (n != 9 || memcmp(line, "version=3", 9) != 0) {
                dp_fail(err, "unsupported manifest");
                return false;
            }
            continue;
        }
        char *name = dp_strndup(line, n);
        if (!name) {
            dp_manifest_free(out);
            dp_fail(err, "out of memory");
            return false;
        }
        bool duplicate = false;
        for (size_t i = 0; i < out->count; i++) duplicate |= strcmp(out->names[i], name) == 0;
        if (out->count == DP_MAX_MANIFEST_MEMBERS || memchr(line, 0, n) ||
            !dp_valid_manifest_name(name) || duplicate) {
            free(name);
            dp_manifest_free(out);
            dp_fail(err, "unsafe or duplicate manifest member");
            return false;
        }
        char **grown = realloc(out->names, (out->count + 1) * sizeof(*grown));
        if (!grown) {
            free(name);
            dp_manifest_free(out);
            dp_fail(err, "out of memory");
            return false;
        }
        out->names = grown;
        out->names[out->count++] = name;
        has_transaction |= strcmp(name, "deploy/transaction.sh") == 0;
        has_manifest |= strcmp(name, "deploy/manifest-v3.txt") == 0;
    }
    if (first) {
        dp_fail(err, "unsupported manifest");
        return false;
    }
    if (!has_transaction || !has_manifest) {
        dp_manifest_free(out);
        dp_fail(err, "manifest is missing transaction members");
        return false;
    }
    return true;
}

bool dp_load_manifest(const char *path, dp_manifest *out, dp_err *err) {
    out->names = NULL;
    out->count = 0;
    uint64_t size = 0;
    dp_path_kind kind = dp_path_lstat(path);
    if (kind == DP_PATH_MISSING) {
        dp_fail(err, "%s: No such file or directory (os error 2)", path);
        return false;
    }
    if (kind != DP_PATH_FILE || !dp_file_size(path, &size) || size > 32u * 1024u) {
        dp_fail(err, "manifest is not a bounded regular file");
        return false;
    }
    omt_buf text;
    omt_buf_init(&text, 32u * 1024u + 1);
    bool ok = dp_read_file(path, 32u * 1024u, &text, err);
    if (ok && !omt_utf8_valid((const char *)text.data, text.len)) {
        dp_fail(err, "stream did not contain valid UTF-8");
        ok = false;
    }
    if (ok) ok = dp_parse_manifest((const char *)text.data, text.len, out, err);
    omt_buf_free(&text);
    return ok;
}

bool dp_secure_relative(const char *root, const char *member, omt_buf *out, dp_err *err) {
    if (!dp_valid_manifest_name(member)) {
        dp_fail(err, "unsafe member");
        return false;
    }
    if (DP_ON_WINDOWS) {
        /* Path::join on Windows takes the member's slashes as separators too. */
        omt_buf_puts(out, root);
        size_t len = out->len;
        if (len > 0 && out->data[len - 1] != '/' && out->data[len - 1] != '\\')
            omt_buf_putc(out, '\\');
        for (const char *p = member; *p; p++) omt_buf_putc(out, *p == '/' ? '\\' : (uint8_t)*p);
    } else {
        dp_path_join(out, root, member);
    }
    return !out->failed;
}

/* --------------------------------------------------------------- progress */

void dp_report(const dp_progress *p, const char *text) {
    if (!p || !p->line) return;
    /* One callback per non-empty line, as the Rust frontends split them. */
    const char *line = text;
    while (*line) {
        const char *nl = strchr(line, '\n');
        size_t n = nl ? (size_t)(nl - line) : strlen(line);
        size_t visible = n;
        if (visible > 0 && line[visible - 1] == '\r') visible--;
        if (visible > 0) {
            char *copy = dp_strndup(line, visible);
            if (copy) {
                p->line(p->ctx, copy);
                free(copy);
            }
        }
        if (!nl) break;
        line = nl + 1;
    }
}

void dp_reportf(const dp_progress *p, const char *fmt, ...) {
    omt_buf text;
    omt_buf_init(&text, DP_ERR_LIMIT);
    va_list args;
    va_start(args, fmt);
    omt_buf_vprintf(&text, fmt, args);
    va_end(args);
    dp_report(p, omt_buf_cstr(&text));
    omt_buf_free(&text);
}

/* ---------------------------------------------------------------- capsule */

bool dp_capsule_report(omt_buf *out, dp_err *err) {
    const dp_capsule_member *image = dp_capsule_image();
    if (!image) {
        dp_fail(err, "this deployer was built without " DP_IMAGE_MEMBER);
        return false;
    }
    size_t count;
    dp_capsule_members(&count);
    char digest[65];
    dp_sha256_hex(image->bytes, image->size, digest);
    omt_buf_printf(out, "Embedded capsule: %zu members, " DP_IMAGE_MEMBER " %zu MiB, sha256 %s.",
                   count, image->size / (1024u * 1024u), digest);
    return !out->failed;
}
