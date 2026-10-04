/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * OpenSSH known_hosts, as sshd(8) documents the format: plain, wildcard, and
 * negated host patterns, `[host]:port` for a non-default port, hashed `|1|`
 * entries, and `@revoked` markers. `@cert-authority` lines are skipped, so a
 * host trusted only through a CA is unknown here: this client does not
 * verify certificates.
 *
 * Checking is strict. A matching key is the only success; a host with no
 * entry for the offered key is refused, and the file has to exist.
 */
#include <openssl/core_names.h>
#include <openssl/params.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

#define KNOWN_HOSTS_LIMIT (8u * 1024u * 1024u)

static int b64_value(uint8_t c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

bool ssh_base64_decode(const char *text, size_t len, bool skip_space, omt_buf *out) {
    uint32_t acc = 0;
    int bits = 0;
    size_t pad = 0, symbols = 0;
    for (size_t i = 0; i < len; i++) {
        uint8_t c = (uint8_t)text[i];
        if (skip_space && (c == ' ' || c == '\t' || c == '\r' || c == '\n')) continue;
        if (c == '=') {
            pad++;
            symbols++;
            continue;
        }
        int v = b64_value(c);
        if (v < 0 || pad > 0) return false;
        symbols++;
        acc = (acc << 6) | (uint32_t)v;
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            omt_buf_putc(out, (uint8_t)(acc >> bits));
            acc &= (1u << bits) - 1u;
        }
    }
    if (pad > 2 || symbols % 4 != 0 || acc != 0) return false;
    return !out->failed;
}

static uint8_t lower(uint8_t c) { return c >= 'A' && c <= 'Z' ? (uint8_t)(c + 32) : c; }

/* Glob match with * and ?, case-insensitively, as OpenSSH's match_pattern. */
static bool glob(const char *s, size_t slen, const char *p, size_t plen) {
    size_t si = 0, pi = 0, star = (size_t)-1, mark = 0;
    while (si < slen) {
        if (pi < plen && (p[pi] == '?' || lower((uint8_t)p[pi]) == lower((uint8_t)s[si]))) {
            si++;
            pi++;
        } else if (pi < plen && p[pi] == '*') {
            star = pi++;
            mark = si;
        } else if (star != (size_t)-1) {
            pi = star + 1;
            si = ++mark;
        } else {
            return false;
        }
    }
    while (pi < plen && p[pi] == '*') pi++;
    return pi == plen;
}

static bool hashed_match(const char *entry, size_t len, const char *name) {
    /* |1|base64(salt)|base64(HMAC-SHA1(salt, name)) */
    if (len < 3 || memcmp(entry, "|1|", 3) != 0) return false;
    const char *salt = entry + 3;
    const char *bar = memchr(salt, '|', len - 3);
    if (!bar) return false;
    omt_buf salt_raw, hash_raw;
    omt_buf_init(&salt_raw, 256);
    omt_buf_init(&hash_raw, 256);
    bool ok = ssh_base64_decode(salt, (size_t)(bar - salt), false, &salt_raw) &&
              ssh_base64_decode(bar + 1, len - (size_t)(bar + 1 - entry), false, &hash_raw) &&
              salt_raw.len == 20 && hash_raw.len == 20;
    bool match = false;
    if (ok) {
        uint8_t mac[20];
        size_t mac_len = 0;
        EVP_MAC *impl = EVP_MAC_fetch(NULL, "HMAC", NULL);
        EVP_MAC_CTX *ctx = impl ? EVP_MAC_CTX_new(impl) : NULL;
        char digest[] = "SHA1";
        OSSL_PARAM params[] = {OSSL_PARAM_utf8_string(OSSL_MAC_PARAM_DIGEST, digest, 0),
                               OSSL_PARAM_END};
        if (ctx && EVP_MAC_init(ctx, salt_raw.data, salt_raw.len, params) == 1 &&
            EVP_MAC_update(ctx, (const uint8_t *)name, strlen(name)) == 1 &&
            EVP_MAC_final(ctx, mac, &mac_len, sizeof(mac)) == 1 && mac_len == 20) {
            match = memcmp(mac, hash_raw.data, 20) == 0;
        }
        EVP_MAC_CTX_free(ctx);
        EVP_MAC_free(impl);
    }
    omt_buf_free(&salt_raw);
    omt_buf_free(&hash_raw);
    return match;
}

/* Whether a hostnames field names `name`. A matching negated pattern excludes
 * the line however the other patterns match. */
static bool hosts_match(const char *field, size_t len, const char *name) {
    if (len > 0 && field[0] == '|') return hashed_match(field, len, name);
    bool positive = false;
    size_t start = 0;
    for (size_t i = 0; i <= len; i++) {
        if (i < len && field[i] != ',') continue;
        const char *pat = field + start;
        size_t plen = i - start;
        start = i + 1;
        bool negated = plen > 0 && pat[0] == '!';
        if (negated) {
            pat++;
            plen--;
        }
        if (plen == 0) continue;
        if (glob(name, strlen(name), pat, plen)) {
            if (negated) return false;
            positive = true;
        }
    }
    return positive;
}

/* The next whitespace-separated field of a line. */
static bool next_field(const char **p, const char *end, const char **field, size_t *len) {
    while (*p < end && (**p == ' ' || **p == '\t')) (*p)++;
    if (*p >= end) return false;
    *field = *p;
    while (*p < end && **p != ' ' && **p != '\t') (*p)++;
    *len = (size_t)(*p - *field);
    return true;
}

/* The key type a blob declares, or an empty span. */
static omt_span blob_type(omt_span blob) {
    omt_span r = blob, type = {NULL, 0};
    if (!sr_string(&r, &type)) type.len = 0;
    return type;
}

static void lookup_name(const char *host, uint16_t port, char *out, size_t size) {
    char lowered[256];
    size_t n = 0;
    for (; host[n] && n + 1 < sizeof(lowered); n++) lowered[n] = (char)lower((uint8_t)host[n]);
    lowered[n] = 0;
    if (port == 22) {
        omt_strlcpy(out, lowered, size);
    } else {
        omt_snprintf(out, size, "[%s]:%u", lowered, (unsigned)port);
    }
}

bool ssh_known_hosts_parse(const char *text, size_t len, const char *host, uint16_t port,
                           omt_span key_blob, ssh_host_status *status, omt_buf *types) {
    char name[300];
    lookup_name(host, port, name, sizeof(name));
    omt_span offered_type = blob_type(key_blob);
    bool known = false, revoked = false, same_type = false;
    omt_buf key;
    omt_buf_init(&key, 64u * 1024u);
    size_t pos = 0;
    while (pos < len) {
        const char *line = text + pos;
        const char *nl = memchr(line, '\n', len - pos);
        const char *end = nl ? nl : text + len;
        pos = (size_t)(end - text) + (nl ? 1 : 0);
        if (end > line && end[-1] == '\r') end--;
        const char *p = line;
        const char *field;
        size_t flen;
        if (!next_field(&p, end, &field, &flen) || field[0] == '#') continue;
        bool is_revoked = false;
        if (field[0] == '@') {
            if (flen == 8 && memcmp(field, "@revoked", 8) == 0) {
                is_revoked = true;
            } else {
                continue; /* @cert-authority and anything unknown */
            }
            if (!next_field(&p, end, &field, &flen)) continue;
        }
        const char *hosts = field;
        size_t hosts_len = flen;
        const char *ktype, *kdata;
        size_t ktype_len, kdata_len;
        if (!next_field(&p, end, &ktype, &ktype_len) || !next_field(&p, end, &kdata, &kdata_len)) {
            continue;
        }
        omt_buf_clear(&key);
        if (!ssh_base64_decode(kdata, kdata_len, false, &key)) continue;
        omt_span line_key = omt_span_of(key.data, key.len);
        omt_span line_type = blob_type(line_key);
        bool equal = key_blob.len > 0 && line_key.len == key_blob.len &&
                     memcmp(line_key.p, key_blob.p, key_blob.len) == 0;
        if (is_revoked) {
            /* A revoked key is refused for every host it might appear for. */
            revoked |= equal;
            continue;
        }
        if (!hosts_match(hosts, hosts_len, name)) continue;
        if (equal) known = true;
        if (offered_type.len > 0 && line_type.len == offered_type.len &&
            memcmp(line_type.p, offered_type.p, offered_type.len) == 0) {
            same_type = true;
        }
        if (types && line_type.len > 0) {
            if (types->len) omt_buf_putc(types, ',');
            omt_buf_append(types, line_type.p, line_type.len);
        }
    }
    omt_buf_free(&key);
    if (revoked) {
        *status = SSH_HOST_REVOKED;
    } else if (known) {
        *status = SSH_HOST_KNOWN;
    } else if (same_type) {
        *status = SSH_HOST_CHANGED;
    } else {
        *status = SSH_HOST_UNKNOWN;
    }
    return true;
}

static bool read_known_hosts(const char *path, omt_buf *text, dp_err *err) {
    if (!dp_is_file(path)) {
        dp_fail(err,
                "OpenSSH known_hosts was not found at %s. Connect with ssh first and verify the "
                "Raspberry Pi host key.",
                path);
        return false;
    }
    return dp_read_file(path, KNOWN_HOSTS_LIMIT, text, err);
}

bool ssh_known_hosts_check(const char *path, const char *host, uint16_t port, omt_span key_blob,
                           ssh_host_status *status, dp_err *err) {
    omt_buf text;
    omt_buf_init(&text, KNOWN_HOSTS_LIMIT + 1);
    bool ok = read_known_hosts(path, &text, err) &&
              ssh_known_hosts_parse((const char *)text.data, text.len, host, port, key_blob, status,
                                    NULL);
    omt_buf_free(&text);
    return ok;
}

bool ssh_known_hosts_types(const char *path, const char *host, uint16_t port, omt_buf *types,
                           dp_err *err) {
    omt_buf text;
    omt_buf_init(&text, KNOWN_HOSTS_LIMIT + 1);
    ssh_host_status ignored;
    omt_span none = {NULL, 0};
    bool ok =
        read_known_hosts(path, &text, err) &&
        ssh_known_hosts_parse((const char *)text.data, text.len, host, port, none, &ignored, types);
    omt_buf_free(&text);
    return ok;
}
