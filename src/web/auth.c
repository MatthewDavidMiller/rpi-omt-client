/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/auth.h"

#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/rand.h"
#include "crypto/crypto.h"

#define SECRET_LIMIT 256
#define PASSWORD_LIMIT (16 * 1024)
#define REGISTRY_LIMIT (64 * 1024)
#define MAXIMUM_SESSIONS 64
#define PBKDF2_ITERATIONS 600000u

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

static bool hex_decode(const char *s, size_t len, uint8_t *out, size_t cap, size_t *out_len) {
    if (len % 2 != 0 || len / 2 > cap) return false;
    for (size_t i = 0; i < len; i += 2) {
        int hi = hexval(s[i]), lo = hexval(s[i + 1]);
        if (hi < 0 || lo < 0) return false;
        out[i / 2] = (uint8_t)(hi << 4 | lo);
    }
    *out_len = len / 2;
    return true;
}

static const char *find_char(const char *s, size_t len, char c) { return memchr(s, c, len); }

static bool parse_u32(const char *s, size_t len, uint32_t *out) {
    if (len > 0 && s[0] == '+') {
        s++;
        len--;
    }
    uint64_t v;
    if (!omt_parse_u64(s, len, UINT32_MAX, &v)) return false;
    *out = (uint32_t)v;
    return true;
}

void omt_password_free(omt_password *p) {
    if (p->plain) omt_cleanse(p->plain, p->plain_len);
    free(p->plain);
    free(p->salt);
    memset(p, 0, sizeof(*p));
}

static bool copy_salt(omt_password *out, const char *salt, size_t len) {
    out->salt = malloc(len ? len : 1);
    if (!out->salt) return false;
    if (len) memcpy(out->salt, salt, len);
    out->salt_len = len;
    return true;
}

bool omt_password_parse(const char *value, size_t len, omt_password *out, omt_err *err) {
    memset(out, 0, sizeof(*out));
    if (len >= 14 && memcmp(value, "pbkdf2:sha256:", 14) == 0) {
        const char *rest = value + 14;
        size_t rlen = len - 14;
        const char *d1 = find_char(rest, rlen, '$');
        const char *d2 = d1 ? find_char(d1 + 1, rlen - (size_t)(d1 + 1 - rest), '$') : NULL;
        if (!d1 || !d2) {
            omt_err_set(err, "invalid PBKDF2 password hash");
            return false;
        }
        if (!parse_u32(rest, (size_t)(d1 - rest), &out->iterations) || out->iterations == 0) {
            omt_err_set(err, "invalid PBKDF2 iterations");
            return false;
        }
        const char *digest = d2 + 1;
        size_t digest_len = rlen - (size_t)(digest - rest);
        if (!hex_decode(digest, digest_len, out->digest, sizeof(out->digest), &out->digest_len)) {
            omt_err_set(err, "invalid hexadecimal value");
            return false;
        }
        if (out->digest_len != 32) {
            omt_err_set(err, "invalid PBKDF2 SHA-256 digest length");
            return false;
        }
        out->kind = PASSWORD_PBKDF2;
        if (!copy_salt(out, d1 + 1, (size_t)(d2 - d1 - 1))) {
            omt_err_set(err, "out of memory");
            return false;
        }
        return true;
    }
    if (len >= 7 && memcmp(value, "scrypt:", 7) == 0) {
        const char *rest = value + 7;
        size_t rlen = len - 7;
        const char *d1 = find_char(rest, rlen, '$');
        if (!d1) {
            omt_err_set(err, "invalid scrypt password hash");
            return false;
        }
        /* Exactly N:r:p, each a u32. */
        uint32_t params[3];
        const char *p = rest, *end = d1;
        for (int i = 0; i < 3; i++) {
            if (p > end) {
                omt_err_set(err, i == 0   ? "missing scrypt N"
                                 : i == 1 ? "missing scrypt r"
                                          : "missing scrypt p");
                return false;
            }
            const char *colon = find_char(p, (size_t)(end - p), ':');
            const char *stop = colon ? colon : end;
            if (!parse_u32(p, (size_t)(stop - p), &params[i])) {
                omt_err_set(err, i == 0   ? "invalid scrypt N"
                                 : i == 1 ? "invalid scrypt r"
                                          : "invalid scrypt p");
                return false;
            }
            p = colon ? colon + 1 : end + 1;
            if (i == 2 && colon) {
                omt_err_set(err, "invalid scrypt parameters");
                return false;
            }
        }
        uint32_t n = params[0];
        if (n < 2 || (n & (n - 1)) != 0) {
            omt_err_set(err, "invalid scrypt parameters");
            return false;
        }
        const char *d2 = find_char(d1 + 1, rlen - (size_t)(d1 + 1 - rest), '$');
        if (!d2) {
            omt_err_set(err, "invalid scrypt password hash");
            return false;
        }
        const char *digest = d2 + 1;
        size_t digest_len = rlen - (size_t)(digest - rest);
        size_t out_len = digest_len / 2;
        if (params[1] == 0 || params[2] == 0 || (uint64_t)params[1] * params[2] >= (1u << 30) ||
            out_len < 10 || out_len > 64) {
            omt_err_set(err, "unsupported scrypt parameters");
            return false;
        }
        if (!hex_decode(digest, digest_len, out->digest, sizeof(out->digest), &out->digest_len)) {
            omt_err_set(err, "invalid hexadecimal value");
            return false;
        }
        out->kind = PASSWORD_SCRYPT;
        out->n = n;
        out->r = params[1];
        out->p = params[2];
        if (!copy_salt(out, d1 + 1, (size_t)(d2 - d1 - 1))) {
            omt_err_set(err, "out of memory");
            return false;
        }
        return true;
    }
    if ((len >= 7 && memcmp(value, "argon2:", 7) == 0) ||
        (len >= 7 && memcmp(value, "pbkdf2:", 7) == 0)) {
        omt_err_set(err, "The Web GUI password hash uses an unsupported format");
        return false;
    }
    out->kind = PASSWORD_PLAIN;
    out->plain = malloc(len + 1);
    if (!out->plain) {
        omt_err_set(err, "out of memory");
        return false;
    }
    memcpy(out->plain, value, len);
    out->plain[len] = 0;
    out->plain_len = len;
    return true;
}

bool omt_password_verify(const omt_password *p, const char *supplied, size_t len) {
    uint8_t actual[64];
    bool ok = false;
    switch (p->kind) {
    case PASSWORD_PLAIN: return omt_ct_equal(p->plain, p->plain_len, supplied, len);
    case PASSWORD_PBKDF2:
        ok = omt_pbkdf2_sha256(supplied, len, p->salt, p->salt_len, p->iterations, actual,
                               p->digest_len);
        break;
    case PASSWORD_SCRYPT:
        ok = omt_scrypt(supplied, len, p->salt, p->salt_len, p->n, p->r, p->p, actual,
                        p->digest_len);
        break;
    }
    bool equal = ok && omt_ct_equal(actual, p->digest_len, p->digest, p->digest_len);
    omt_cleanse(actual, sizeof(actual));
    return equal;
}

/* ------------------------------------------------------------- registry */

typedef struct {
    char key[OMT_SESSION_ID_HEX + 1];
    uint64_t expires_at;
    char password_digest[OMT_SESSION_ID_HEX + 1];
} record;

typedef struct {
    record items[MAXIMUM_SESSIONS + 1];
    size_t count;
} registry;

static uint64_t now_epoch(void) {
    time_t t = time(NULL);
    return t < 0 ? 0 : (uint64_t)t;
}

static bool registry_path(const omt_auth *a, char *out) {
    return omt_snprintf(out, OMT_PATH_MAX, "%s/web_sessions.json", a->settings->config_dir);
}

/* Any unreadable, malformed, or foreign-version registry reads as empty: a
 * corrupt file costs the operator a login, never access. */
static void read_registry(const omt_auth *a, registry *r) {
    r->count = 0;
    char path[OMT_PATH_MAX];
    if (!registry_path(a, path)) return;
    omt_buf text;
    omt_err err;
    if (omt_read_text(path, REGISTRY_LIMIT, &text, &err) != OMT_READ_OK) return;
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, omt_buf_cstr(&text), text.len, 0);
    const char *top[] = {"version", "sessions"};
    uint64_t version;
    const omt_json *sessions = omt_json_get(root, "sessions");
    bool ok = root && omt_json_only_keys(root, top, 2) &&
              omt_json_as_u64(omt_json_get(root, "version"), &version) && version <= 255 &&
              sessions && sessions->type == OMT_JSON_OBJECT && version == 2;
    for (size_t i = 0; ok && i < sessions->count; i++) {
        const omt_json *rec = sessions->items[i];
        const char *fields[] = {"expires_at", "password_digest"};
        uint64_t expires;
        const char *digest = omt_json_as_str(omt_json_get(rec, "password_digest"));
        ok = omt_json_only_keys(rec, fields, 2) &&
             omt_json_as_u64(omt_json_get(rec, "expires_at"), &expires) && digest &&
             strlen(digest) <= OMT_SESSION_ID_HEX && sessions->key_lens[i] <= OMT_SESSION_ID_HEX &&
             r->count < MAXIMUM_SESSIONS + 1;
        if (!ok) break;
        record *out = &r->items[r->count++];
        memcpy(out->key, sessions->keys[i], sessions->key_lens[i]);
        out->key[sessions->key_lens[i]] = 0;
        out->expires_at = expires;
        omt_strlcpy(out->password_digest, digest, sizeof(out->password_digest));
    }
    if (!ok) r->count = 0;
    if (root) omt_json_doc_free(&doc);
    omt_buf_free(&text);
}

static int by_key(const void *x, const void *y) {
    return strcmp(((const record *)x)->key, ((const record *)y)->key);
}

static bool write_registry(const omt_auth *a, registry *r, omt_err *err) {
    /* Keys sorted, as the BTreeMap the Rust frontend serialized. */
    qsort(r->items, r->count, sizeof(record), by_key);
    omt_buf out;
    omt_buf_init(&out, REGISTRY_LIMIT);
    omt_buf_puts(&out, "{\"version\":2,\"sessions\":{");
    for (size_t i = 0; i < r->count; i++) {
        if (i) omt_buf_putc(&out, ',');
        omt_json_write_cstr(&out, r->items[i].key);
        omt_buf_printf(&out, ":{\"expires_at\":%llu,\"password_digest\":",
                       (unsigned long long)r->items[i].expires_at);
        omt_json_write_cstr(&out, r->items[i].password_digest);
        omt_buf_putc(&out, '}');
    }
    omt_buf_puts(&out, "}}\n");
    char path[OMT_PATH_MAX];
    bool ok = !out.failed && registry_path(a, path) &&
              omt_atomic_replace(path, out.data, out.len, REGISTRY_LIMIT, err);
    if (out.failed) omt_err_set(err, "session registry exceeds its size limit");
    omt_buf_free(&out);
    return ok;
}

static bool keyed_hex(const uint8_t *key, size_t key_len, const void *data, size_t len,
                      char out[OMT_SESSION_ID_HEX + 1]) {
    omt_buf b;
    omt_buf_init(&b, OMT_SESSION_ID_HEX);
    bool ok = omt_hmac_sha256_hex(key, key_len, data, len, &b) && b.len == OMT_SESSION_ID_HEX;
    if (ok) memcpy(out, b.data, OMT_SESSION_ID_HEX + 1);
    omt_buf_free(&b);
    return ok;
}

static bool session_digest(const omt_auth *a, const char *session_id,
                           char out[OMT_SESSION_ID_HEX + 1]) {
    return keyed_hex(a->secret, a->secret_len, session_id, strlen(session_id), out);
}

bool omt_auth_load(omt_auth *a, const omt_web_settings *s, omt_err *err) {
    memset(a, 0, sizeof(*a));
    a->settings = s;
    char path[OMT_PATH_MAX];
    if (!omt_snprintf(path, sizeof(path), "%s/web_secret", s->config_dir)) {
        omt_err_set(err, "The Web secret is missing or unsafe");
        return false;
    }
    omt_buf secret;
    omt_read_result r = omt_read_text(path, SECRET_LIMIT, &secret, err);
    if (r == OMT_READ_ERROR) return false;
    omt_span trimmed = omt_utf8_trim(omt_buf_cstr(&secret), secret.len);
    if (r == OMT_READ_MISSING || trimmed.len == 0) {
        omt_buf_free_secret(&secret);
        omt_err_set(err, "The Web secret is missing or unsafe");
        return false;
    }
    memcpy(a->secret, trimmed.p, trimmed.len);
    a->secret_len = trimmed.len;
    omt_buf_free_secret(&secret);

    omt_buf password;
    omt_buf_init(&password, PASSWORD_LIMIT);
    const char *env = getenv("OMT_WEB_PASSWORD");
    omt_span text;
    if (env && *env) {
        omt_buf_puts(&password, env);
        text = omt_span_of(password.data, password.len);
    } else {
        r = omt_read_text(s->password_file, PASSWORD_LIMIT, &password, err);
        if (r == OMT_READ_ERROR) return false;
        text = omt_utf8_trim(omt_buf_cstr(&password), password.len);
        if (r == OMT_READ_MISSING || text.len == 0) {
            omt_buf_free_secret(&password);
            omt_err_set(err, "The Web GUI password file is missing or unsafe");
            return false;
        }
    }
    bool ok = keyed_hex(a->secret, a->secret_len, text.p, text.len, a->password_digest) &&
              omt_password_parse((const char *)text.p, text.len, &a->password, err);
    omt_buf_free_secret(&password);
    return ok;
}

void omt_auth_free(omt_auth *a) {
    omt_password_free(&a->password);
    omt_cleanse(a->secret, sizeof(a->secret));
}

static int by_expiry_then_key(const record *x, const record *y) {
    if (x->expires_at != y->expires_at) return x->expires_at < y->expires_at ? -1 : 1;
    return strcmp(x->key, y->key);
}

int omt_auth_authenticate(omt_auth *a, const char *password, size_t len, const char *previous,
                          char session_out[OMT_SESSION_ID_HEX + 1], omt_err *err) {
    if (!omt_password_verify(&a->password, password, len)) return 0;
    omt_buf id;
    omt_buf_init(&id, OMT_SESSION_ID_HEX);
    if (!omt_random_hex(&id, 32, err)) {
        omt_buf_free(&id);
        return -1;
    }
    memcpy(session_out, id.data, OMT_SESSION_ID_HEX + 1);
    omt_buf_free(&id);
    uint64_t now = now_epoch();
    registry *reg = malloc(sizeof(*reg));
    if (!reg) {
        omt_err_set(err, "out of memory");
        return -1;
    }
    read_registry(a, reg);
    size_t kept = 0;
    char previous_digest[OMT_SESSION_ID_HEX + 1] = "";
    if (previous && !session_digest(a, previous, previous_digest)) previous_digest[0] = 0;
    for (size_t i = 0; i < reg->count; i++) {
        const record *rec = &reg->items[i];
        if (rec->expires_at > now && strcmp(rec->password_digest, a->password_digest) == 0 &&
            !(previous_digest[0] && strcmp(rec->key, previous_digest) == 0))
            reg->items[kept++] = *rec;
    }
    reg->count = kept;
    record fresh;
    memset(&fresh, 0, sizeof(fresh));
    if (!session_digest(a, session_out, fresh.key)) {
        free(reg);
        omt_err_set(err, "unable to derive the session key");
        return -1;
    }
    fresh.expires_at = now > UINT64_MAX - a->settings->session_lifetime_s
                           ? UINT64_MAX
                           : now + a->settings->session_lifetime_s;
    omt_strlcpy(fresh.password_digest, a->password_digest, sizeof(fresh.password_digest));
    /* Replace an equal key (astronomically unlikely) rather than duplicate it. */
    size_t at = reg->count;
    for (size_t i = 0; i < reg->count; i++)
        if (strcmp(reg->items[i].key, fresh.key) == 0) at = i;
    reg->items[at] = fresh;
    if (at == reg->count) reg->count++;
    /* Evict the soonest-expiring sessions past the cap. */
    while (reg->count > MAXIMUM_SESSIONS) {
        size_t oldest = 0;
        for (size_t i = 1; i < reg->count; i++)
            if (by_expiry_then_key(&reg->items[i], &reg->items[oldest]) < 0) oldest = i;
        reg->items[oldest] = reg->items[reg->count - 1];
        reg->count--;
    }
    bool ok = write_registry(a, reg, err);
    free(reg);
    return ok ? 1 : -1;
}

bool omt_auth_is_current(omt_auth *a, const char *session_id) {
    char digest[OMT_SESSION_ID_HEX + 1];
    if (!session_digest(a, session_id, digest)) return false;
    registry *reg = malloc(sizeof(*reg));
    if (!reg) return false;
    read_registry(a, reg);
    bool current = false;
    uint64_t now = now_epoch();
    for (size_t i = 0; i < reg->count && !current; i++)
        current = strcmp(reg->items[i].key, digest) == 0 && reg->items[i].expires_at > now &&
                  strcmp(reg->items[i].password_digest, a->password_digest) == 0;
    free(reg);
    return current;
}

bool omt_auth_revoke(omt_auth *a, const char *session_id, omt_err *err) {
    char digest[OMT_SESSION_ID_HEX + 1];
    if (!session_digest(a, session_id, digest)) {
        omt_err_set(err, "unable to derive the session key");
        return false;
    }
    registry *reg = malloc(sizeof(*reg));
    if (!reg) {
        omt_err_set(err, "out of memory");
        return false;
    }
    read_registry(a, reg);
    bool removed = false;
    for (size_t i = 0; i < reg->count; i++)
        if (strcmp(reg->items[i].key, digest) == 0) {
            reg->items[i] = reg->items[reg->count - 1];
            reg->count--;
            removed = true;
            break;
        }
    bool ok = !removed || write_registry(a, reg, err);
    free(reg);
    return ok;
}

bool omt_auth_csrf_token(const omt_auth *a, const char *scope, const char *nonce,
                         char out[OMT_SESSION_ID_HEX + 1]) {
    omt_buf msg;
    omt_buf_init(&msg, 4096);
    omt_buf_puts(&msg, "csrf");
    omt_buf_putc(&msg, 0);
    omt_buf_puts(&msg, scope);
    omt_buf_putc(&msg, 0);
    omt_buf_puts(&msg, nonce);
    bool ok = !msg.failed && keyed_hex(a->secret, a->secret_len, msg.data, msg.len, out);
    omt_buf_free(&msg);
    return ok;
}

bool omt_auth_verify_csrf(const omt_auth *a, const char *scope, const char *nonce,
                          const char *token) {
    char expected[OMT_SESSION_ID_HEX + 1];
    return omt_auth_csrf_token(a, scope, nonce, expected) &&
           omt_ct_equal(expected, strlen(expected), token, strlen(token));
}

bool omt_validate_new_password(const char *password, size_t len, omt_err *err) {
    if (len < OMT_MINIMUM_PASSWORD_BYTES || len > OMT_MAXIMUM_PASSWORD_BYTES) {
        omt_err_set(err, "The Web GUI password must contain %d-%d UTF-8 bytes",
                    OMT_MINIMUM_PASSWORD_BYTES, OMT_MAXIMUM_PASSWORD_BYTES);
        return false;
    }
    size_t i = 0;
    while (i < len) {
        uint32_t cp = omt_utf8_next(password, len, &i);
        if (cp < 0x20 || (cp >= 0x7F && cp <= 0x9F)) {
            omt_err_set(err, "The Web GUI password must not contain control characters");
            return false;
        }
    }
    return true;
}

bool omt_encode_password(const char *password, size_t len, omt_buf *out, omt_err *err) {
    omt_buf salt;
    omt_buf_init(&salt, 64);
    if (!omt_random_hex(&salt, 16, err)) {
        omt_buf_free(&salt);
        return false;
    }
    uint8_t digest[32];
    bool ok = omt_pbkdf2_sha256(password, len, salt.data, salt.len, PBKDF2_ITERATIONS, digest,
                                sizeof(digest));
    if (ok) {
        omt_buf_printf(out, "pbkdf2:sha256:%u$%s$", PBKDF2_ITERATIONS, omt_buf_cstr(&salt));
        omt_buf_hex(out, digest, sizeof(digest));
        omt_buf_putc(out, '\n');
        ok = !out->failed;
    }
    if (!ok) omt_err_set(err, "unable to derive the password hash");
    omt_cleanse(digest, sizeof(digest));
    omt_buf_free(&salt);
    return ok;
}

bool omt_replace_password(const omt_web_settings *s, const char *password, size_t len,
                          omt_err *err) {
    const char *env = getenv("OMT_WEB_PASSWORD");
    if (env && *env) {
        omt_err_set(err, "OMT_WEB_PASSWORD overrides the password file; remove the emergency "
                         "override first");
        return false;
    }
    if (!omt_validate_new_password(password, len, err)) return false;
    omt_buf encoded;
    omt_buf_init(&encoded, PASSWORD_LIMIT);
    bool ok = omt_encode_password(password, len, &encoded, err) &&
              omt_atomic_replace(s->password_file, encoded.data, encoded.len, PASSWORD_LIMIT, err);
    omt_buf_free_secret(&encoded);
    return ok;
}

bool omt_auth_initialize(const omt_web_settings *s, omt_buf *generated, omt_err *err) {
    if (!omt_mkdir_all(s->config_dir, 0777, err)) return false;
    char secret_path[OMT_PATH_MAX], legacy_path[OMT_PATH_MAX];
    if (!omt_snprintf(secret_path, sizeof(secret_path), "%s/web_secret", s->config_dir) ||
        !omt_snprintf(legacy_path, sizeof(legacy_path), "%s/flask_secret", s->config_dir)) {
        omt_err_set(err, "configuration path too long");
        return false;
    }
    omt_buf secret;
    omt_read_result r = omt_read_text(secret_path, SECRET_LIMIT, &secret, err);
    if (r == OMT_READ_ERROR) return false;
    omt_span current = omt_utf8_trim(omt_buf_cstr(&secret), secret.len);
    if (r == OMT_READ_MISSING || current.len == 0) {
        omt_buf legacy, fresh;
        omt_buf_init(&fresh, SECRET_LIMIT);
        r = omt_read_text(legacy_path, SECRET_LIMIT, &legacy, err);
        if (r == OMT_READ_ERROR) {
            omt_buf_free_secret(&secret);
            return false;
        }
        omt_span old = omt_utf8_trim(omt_buf_cstr(&legacy), legacy.len);
        bool ok;
        if (r == OMT_READ_OK && old.len) {
            omt_buf_append(&fresh, old.p, old.len);
            ok = true;
        } else {
            ok = omt_random_hex(&fresh, 32, err);
        }
        omt_buf_putc(&fresh, '\n');
        ok = ok && !fresh.failed &&
             omt_atomic_replace(secret_path, fresh.data, fresh.len, SECRET_LIMIT, err);
        omt_buf_free_secret(&legacy);
        omt_buf_free_secret(&fresh);
        if (!ok) {
            omt_buf_free_secret(&secret);
            return false;
        }
    }
    omt_buf_free_secret(&secret);
    omt_buf password;
    r = omt_read_text(s->password_file, PASSWORD_LIMIT, &password, err);
    if (r == OMT_READ_ERROR) return false;
    omt_span existing = omt_utf8_trim(omt_buf_cstr(&password), password.len);
    bool present = r == OMT_READ_OK && existing.len > 0;
    omt_buf_free_secret(&password);
    if (present) return true;
    if (!omt_random_hex(generated, 16, err)) return false;
    omt_buf encoded;
    omt_buf_init(&encoded, PASSWORD_LIMIT);
    bool ok = omt_encode_password(omt_buf_cstr(generated), generated->len, &encoded, err) &&
              omt_atomic_replace(s->password_file, encoded.data, encoded.len, PASSWORD_LIMIT, err);
    omt_buf_free_secret(&encoded);
    return ok;
}

void omt_remove_legacy_secret(const omt_web_settings *s) {
    char legacy[OMT_PATH_MAX];
    struct stat st;
    if (omt_snprintf(legacy, sizeof(legacy), "%s/flask_secret", s->config_dir) &&
        stat(legacy, &st) == 0 && S_ISREG(st.st_mode))
        unlink(legacy);
}
