/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * A bounded HTTPS GET for the one download the deployer makes: HTTPS only,
 * TLS 1.2 or later with the system's trust roots and hostname verification,
 * at most five redirects, a byte ceiling, and one deadline for the whole
 * exchange. The response is the caller's to verify; the SD-card path pins it
 * to a SHA-512.
 */
#include <openssl/err.h>
#include <openssl/ssl.h>
#include <openssl/x509v3.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "common/version.h"
#include "deploy/core/deploy.h"
#include "deploy/core/https.h"

#define MAX_HEADER_BYTES (64u * 1024u)

typedef struct {
    char host[256];
    uint16_t port;
    char *path;
} url_parts;

static bool parse_url(const char *url, url_parts *out, dp_err *err) {
    memset(out, 0, sizeof(*out));
    size_t url_len = strlen(url);
    if (url_len < 8 || memcmp(url, "https://", 8) != 0) {
        dp_fail(err, "only https:// URLs are allowed: %s", url);
        return false;
    }
    const char *authority = url + 8;
    size_t alen = strcspn(authority, "/?#");
    const char *rest = authority + alen;
    const char *colon = memchr(authority, ':', alen);
    size_t hlen = colon ? (size_t)(colon - authority) : alen;
    if (hlen == 0 || hlen >= sizeof(out->host) || memchr(authority, '@', alen) ||
        memchr(authority, '[', alen)) {
        dp_fail(err, "unsupported URL: %s", url);
        return false;
    }
    memcpy(out->host, authority, hlen);
    out->host[hlen] = 0;
    out->port = 443;
    if (colon) {
        uint64_t port;
        if (!omt_parse_u64(colon + 1, alen - hlen - 1, 65535, &port) || port == 0) {
            dp_fail(err, "unsupported URL: %s", url);
            return false;
        }
        out->port = (uint16_t)port;
    }
    size_t plen = strcspn(rest, "#");
    omt_buf path;
    omt_buf_init(&path, 8192);
    if (plen == 0 || rest[0] != '/') omt_buf_putc(&path, '/');
    omt_buf_append(&path, rest, plen);
    for (size_t i = 0; i < path.len; i++) {
        if (path.data[i] <= 0x20 || path.data[i] >= 0x7f) path.failed = true;
    }
    out->path = path.failed ? NULL : dp_strdup(omt_buf_cstr(&path));
    omt_buf_free(&path);
    if (!out->path) {
        dp_fail(err, "unsupported URL: %s", url);
        return false;
    }
    return true;
}

typedef struct {
    SSL *ssl;
    dp_sock sock;
    uint64_t deadline;
    const dp_cancel *cancel;
} conn;

/* Retries an SSL operation until it completes, waiting on the socket. */
static int io_wait(conn *c, int rc, dp_err *err) {
    int code = SSL_get_error(c->ssl, rc);
    if (code != SSL_ERROR_WANT_READ && code != SSL_ERROR_WANT_WRITE) return code;
    if (dp_cancelled(c->cancel)) {
        dp_fail_cancelled(err);
        return -1;
    }
    uint64_t now = dp_now_ms();
    if (now >= c->deadline) {
        dp_fail(err, "timed out");
        return -1;
    }
    uint64_t left = c->deadline - now;
    int ready =
        dp_sock_wait(c->sock, code == SSL_ERROR_WANT_WRITE, left > 200 ? 200u : (uint32_t)left);
    if (ready < 0) {
        dp_fail(err, "network wait failed");
        return -1;
    }
    return 0; /* try again */
}

static void tls_error(dp_err *err, const char *what) {
    unsigned long e = ERR_get_error();
    char text[256] = "";
    if (e) ERR_error_string_n(e, text, sizeof(text));
    dp_fail(err, "%s%s%s", what, text[0] ? ": " : "", text);
    ERR_clear_error();
}

static bool tls_connect(conn *c, SSL_CTX *ctx, const char *host, dp_err *err) {
    c->ssl = SSL_new(ctx);
    /* The SNI macro casts its argument to void *; hand it a mutable copy. */
    char name[256];
    omt_strlcpy(name, host, sizeof(name));
    if (!c->ssl || SSL_set_fd(c->ssl, (int)c->sock) != 1 ||
        SSL_set_tlsext_host_name(c->ssl, name) != 1 || SSL_set1_host(c->ssl, host) != 1) {
        tls_error(err, "TLS setup failed");
        return false;
    }
    for (;;) {
        int rc = SSL_connect(c->ssl);
        if (rc == 1) return true;
        int code = io_wait(c, rc, err);
        if (code == 0) continue;
        if (code > 0) {
            long verify = SSL_get_verify_result(c->ssl);
            if (verify != X509_V_OK) {
                dp_fail(err, "TLS certificate verification failed: %s",
                        X509_verify_cert_error_string(verify));
            } else {
                tls_error(err, "TLS handshake failed");
            }
        }
        return false;
    }
}

static bool tls_write(conn *c, const void *data, size_t len, dp_err *err) {
    const uint8_t *p = data;
    while (len > 0) {
        size_t written = 0;
        int rc = SSL_write_ex(c->ssl, p, len, &written);
        if (rc == 1) {
            p += written;
            len -= written;
            continue;
        }
        int code = io_wait(c, rc, err);
        if (code == 0) continue;
        if (code > 0) tls_error(err, "TLS write failed");
        return false;
    }
    return true;
}

/* Bytes read, 0 at end of stream, -1 on error. */
static long tls_read(conn *c, uint8_t *buf, size_t len, dp_err *err) {
    for (;;) {
        size_t got = 0;
        int rc = SSL_read_ex(c->ssl, buf, len, &got);
        if (rc == 1) return (long)got;
        int code = SSL_get_error(c->ssl, rc);
        if (code == SSL_ERROR_ZERO_RETURN) return 0;
        if (code == SSL_ERROR_SYSCALL || code == SSL_ERROR_SSL) {
            /* A peer that closes without close_notify: the caller decides
             * whether the body was complete. */
            unsigned long e = ERR_peek_error();
            if (code == SSL_ERROR_SYSCALL ||
                ERR_GET_REASON(e) == SSL_R_UNEXPECTED_EOF_WHILE_READING) {
                ERR_clear_error();
                return 0;
            }
        }
        code = io_wait(c, rc, err);
        if (code == 0) continue;
        if (code > 0) tls_error(err, "TLS read failed");
        return -1;
    }
}

static const char *header_value(const char *headers, const char *name, size_t *len) {
    size_t n = strlen(name);
    const char *line = strstr(headers, "\r\n");
    while (line && line[2]) {
        line += 2;
        const char *end = strstr(line, "\r\n");
        if (!end) break;
        if ((size_t)(end - line) > n && line[n] == ':' && omt_ascii_ieq(line, n, name)) {
            omt_span v = omt_utf8_trim(line + n + 1, (size_t)(end - line - (ptrdiff_t)n - 1));
            *len = v.len;
            return (const char *)v.p;
        }
        line = end;
    }
    return NULL;
}

/* Reads a chunked body into out. `have` holds bytes already received. */
static bool read_chunked(conn *c, omt_buf *have, size_t limit, omt_buf *out, dp_err *err) {
    uint8_t chunk[16384];
    for (;;) {
        /* A chunk-size line. */
        uint8_t *nl;
        while (!(nl = have->len ? memchr(have->data, '\n', have->len) : NULL)) {
            if (have->len > 1024) goto malformed;
            long n = tls_read(c, chunk, sizeof(chunk), err);
            if (n <= 0) goto truncated_or_error;
            omt_buf_append(have, chunk, (size_t)n);
        }
        size_t line_len = (size_t)(nl - have->data);
        uint64_t size = 0;
        size_t digits = 0;
        for (; digits < line_len; digits++) {
            uint8_t ch = have->data[digits];
            int v = ch >= '0' && ch <= '9'   ? ch - '0'
                    : ch >= 'a' && ch <= 'f' ? ch - 'a' + 10
                    : ch >= 'A' && ch <= 'F' ? ch - 'A' + 10
                                             : -1;
            if (v < 0) break;
            if (size > (limit >> 4) + 1) goto too_large;
            size = size * 16 + (uint64_t)v;
        }
        if (digits == 0) goto malformed;
        omt_buf_consume(have, line_len + 1);
        if (size == 0) return true; /* trailers are not needed */
        if (out->len + size > limit) goto too_large;
        size_t need = (size_t)size + 2;
        while (have->len < need) {
            long n = tls_read(c, chunk, sizeof(chunk), err);
            if (n <= 0) goto truncated_or_error;
            omt_buf_append(have, chunk, (size_t)n);
            if (have->failed) goto too_large;
        }
        omt_buf_append(out, have->data, (size_t)size);
        omt_buf_consume(have, need);
    }
malformed:
    dp_fail(err, "malformed chunked response");
    return false;
too_large:
    dp_fail(err, "response body exceeds %zu bytes", limit);
    return false;
truncated_or_error:
    if (!err || err->msg.len == 0) dp_fail(err, "the response ended early");
    return false;
}

/* One request. Returns the status; a redirect's target is left in location. */
static bool request_once(SSL_CTX *ctx, const url_parts *u, uint64_t deadline, size_t limit,
                         const dp_cancel *cancel, int *status, omt_buf *location, omt_buf *body,
                         dp_err *err) {
    conn c = {NULL, DP_SOCK_INVALID, deadline, cancel};
    uint64_t now = dp_now_ms();
    if (now >= deadline) {
        dp_fail(err, "timed out");
        return false;
    }
    if (!dp_sock_connect(u->host, u->port, (uint32_t)(deadline - now), &c.sock, err)) return false;
    omt_buf req, head;
    omt_buf_init(&req, 16384);
    omt_buf_init(&head, MAX_HEADER_BYTES + 16384);
    bool ok = tls_connect(&c, ctx, u->host, err);
    if (ok) {
        omt_buf_printf(&req,
                       "GET %s HTTP/1.1\r\nHost: %s\r\nUser-Agent: rpi-omt-deploy/%s\r\n"
                       "Accept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
                       u->path, u->host, omt_version);
        ok = tls_write(&c, req.data, req.len, err);
    }
    /* Headers. */
    size_t header_end = 0;
    uint8_t chunk[16384];
    while (ok && header_end == 0) {
        long n = tls_read(&c, chunk, sizeof(chunk), err);
        if (n <= 0) {
            if (n == 0) dp_fail(err, "the server closed the connection before responding");
            ok = false;
            break;
        }
        omt_buf_append(&head, chunk, (size_t)n);
        for (size_t i = 3; i < head.len; i++) {
            if (memcmp(head.data + i - 3, "\r\n\r\n", 4) == 0) {
                header_end = i + 1;
                break;
            }
        }
        if (header_end == 0 && head.len > MAX_HEADER_BYTES) {
            dp_fail(err, "response headers are too large");
            ok = false;
        }
    }
    if (ok) {
        char *headers = dp_strndup((const char *)head.data, header_end);
        omt_buf rest;
        omt_buf_init(&rest, limit + 16384 * 2);
        omt_buf_append(&rest, head.data + header_end, head.len - header_end);
        uint64_t code = 0;
        if (!headers || strlen(headers) < 12 || !omt_has_prefix(headers, "HTTP/1.") ||
            !omt_parse_u64(headers + 9, 3, 999, &code)) {
            dp_fail(err, "malformed HTTP response");
            ok = false;
        }
        *status = (int)code;
        size_t vlen = 0;
        const char *value;
        if (ok && code >= 300 && code < 400) {
            value = header_value(headers, "Location", &vlen);
            if (value) omt_buf_append(location, value, vlen);
        } else if (ok && code == 200) {
            const char *te = header_value(headers, "Transfer-Encoding", &vlen);
            if (te && vlen == 7 && omt_ascii_ieq(te, 7, "chunked")) {
                ok = read_chunked(&c, &rest, limit, body, err);
            } else {
                value = header_value(headers, "Content-Length", &vlen);
                uint64_t length = 0;
                bool sized = value != NULL;
                if (sized && !omt_parse_u64(value, vlen, UINT64_MAX, &length)) {
                    dp_fail(err, "malformed Content-Length");
                    ok = false;
                }
                if (ok && sized && length > limit) {
                    dp_fail(err, "response body exceeds %zu bytes", limit);
                    ok = false;
                }
                omt_buf_append(body, rest.data, rest.len);
                while (ok && (!sized || body->len < length)) {
                    long n = tls_read(&c, chunk, sizeof(chunk), err);
                    if (n < 0) {
                        ok = false;
                    } else if (n == 0) {
                        if (sized) {
                            dp_fail(err, "the response ended early");
                            ok = false;
                        }
                        break;
                    } else {
                        omt_buf_append(body, chunk, (size_t)n);
                    }
                    if (ok && body->len > limit) {
                        dp_fail(err, "response body exceeds %zu bytes", limit);
                        ok = false;
                    }
                }
                if (ok && sized && body->len != length) {
                    dp_fail(err, "the response is longer than its Content-Length");
                    ok = false;
                }
            }
        }
        free(headers);
        omt_buf_free(&rest);
    }
    if (c.ssl) {
        SSL_shutdown(c.ssl);
        SSL_free(c.ssl);
    }
    dp_sock_close(c.sock);
    omt_buf_free(&req);
    omt_buf_free(&head);
    return ok;
}

static bool resolve_location(const url_parts *base, const char *location, omt_buf *out,
                             dp_err *err) {
    if (omt_has_prefix(location, "https://")) {
        omt_buf_puts(out, location);
    } else if (omt_has_prefix(location, "//")) {
        omt_buf_printf(out, "https:%s", location);
    } else if (location[0] == '/') {
        if (base->port == 443) {
            omt_buf_printf(out, "https://%s%s", base->host, location);
        } else {
            omt_buf_printf(out, "https://%s:%u%s", base->host, (unsigned)base->port, location);
        }
    } else {
        dp_fail(err, "refusing a redirect away from HTTPS: %s", location);
        return false;
    }
    return !out->failed;
}

bool dp_https_get(const char *url, size_t limit, uint32_t timeout_ms, const dp_cancel *cancel,
                  omt_buf *body, dp_err *err) {
    SSL_CTX *ctx = SSL_CTX_new(TLS_client_method());
    if (!ctx || SSL_CTX_set_min_proto_version(ctx, TLS1_2_VERSION) != 1) {
        SSL_CTX_free(ctx);
        tls_error(err, "TLS is unavailable");
        return false;
    }
    SSL_CTX_set_verify(ctx, SSL_VERIFY_PEER, NULL);
    if (!dp_tls_load_roots(ctx, err)) {
        SSL_CTX_free(ctx);
        return false;
    }
    uint64_t deadline = dp_now_ms() + timeout_ms;
    omt_buf current, location;
    omt_buf_init(&current, 16384);
    omt_buf_init(&location, 16384);
    omt_buf_puts(&current, url);
    bool ok = false;
    for (int redirects = 0;; redirects++) {
        url_parts u;
        if (!parse_url(omt_buf_cstr(&current), &u, err)) break;
        int status = 0;
        omt_buf_clear(&location);
        omt_buf_clear(body);
        bool got = request_once(ctx, &u, deadline, limit, cancel, &status, &location, body, err);
        if (got && status >= 300 && status < 400 && status != 304) {
            if (location.len == 0) {
                dp_fail(err, "redirect without a Location");
            } else if (redirects == 5) {
                dp_fail(err, "too many redirects");
            } else {
                omt_buf next;
                omt_buf_init(&next, 16384);
                if (resolve_location(&u, omt_buf_cstr(&location), &next, err)) {
                    omt_buf_clear(&current);
                    omt_buf_append(&current, next.data, next.len);
                    omt_buf_free(&next);
                    free(u.path);
                    continue;
                }
                omt_buf_free(&next);
            }
        } else if (got && status != 200) {
            dp_fail(err, "http status: %d", status);
        } else if (got) {
            ok = !body->failed;
            if (!ok) dp_fail(err, "out of memory");
        }
        free(u.path);
        break;
    }
    omt_buf_free(&current);
    omt_buf_free(&location);
    SSL_CTX_free(ctx);
    return ok;
}
