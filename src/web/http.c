/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/http.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <openssl/err.h>
#include <openssl/ssl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#include "common/proc.h"
#include "common/timefmt.h"

typedef enum { CONN_HANDSHAKE, CONN_READ, CONN_WRITE, CONN_SHUTDOWN } conn_state;

typedef struct {
    int fd;
    SSL *ssl;
    conn_state state;
    short want;
    uint64_t deadline;
    omt_buf in;
    omt_buf out;
    size_t out_pos;
    bool head_parsed;
    size_t head_len;
    size_t content_length;
    bool keep_alive;
    bool close_after;
    bool sent_continue;
    char peer[64];
    omt_http_request req;
} conn;

struct omt_http_server {
    int listener;
    SSL_CTX *ctx;
    size_t max_body;
    omt_http_handler handler;
    void *context;
    conn *conns[OMT_HTTP_MAX_CONNECTIONS];
};

const char *omt_http_reason(int status) {
    switch (status) {
    case 100: return "Continue";
    case 200: return "OK";
    case 202: return "Accepted";
    case 303: return "See Other";
    case 400: return "Bad Request";
    case 404: return "Not Found";
    case 405: return "Method Not Allowed";
    case 408: return "Request Timeout";
    case 411: return "Length Required";
    case 413: return "Payload Too Large";
    case 414: return "URI Too Long";
    case 429: return "Too Many Requests";
    case 431: return "Request Header Fields Too Large";
    case 500: return "Internal Server Error";
    case 501: return "Not Implemented";
    case 503: return "Service Unavailable";
    case 505: return "HTTP Version Not Supported";
    default: return "Unknown";
    }
}

static bool ieq(const char *a, size_t alen, const char *b) { return omt_ascii_ieq(a, alen, b); }

bool omt_http_header_value(const omt_http_request *req, const char *name, char *out, size_t size) {
    for (size_t i = 0; i < req->header_count; i++) {
        const omt_http_header *h = &req->headers[i];
        if (!ieq(h->name, h->name_len, name)) continue;
        if (h->value_len >= size) return false;
        memcpy(out, h->value, h->value_len);
        out[h->value_len] = 0;
        return true;
    }
    return false;
}

void omt_http_add_header(omt_http_response *res, const char *name, const char *value) {
    /* A header value never carries a line break; refuse rather than split. */
    if (strpbrk(name, "\r\n:") || strpbrk(value, "\r\n")) {
        res->headers.failed = true;
        return;
    }
    omt_buf_printf(&res->headers, "%s: %s\r\n", name, value);
}

static bool is_tchar(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
           strchr("!#$%&'*+-.^_`|~", c) != NULL;
}

static const uint8_t *find_crlfcrlf(const uint8_t *data, size_t len) {
    for (size_t i = 0; i + 3 < len; i++)
        if (data[i] == '\r' && data[i + 1] == '\n' && data[i + 2] == '\r' && data[i + 3] == '\n')
            return data + i;
    return NULL;
}

int omt_http_parse_head(const uint8_t *data, size_t len, omt_http_request *req, size_t *head_len,
                        size_t *content_length, bool *keep_alive) {
    const uint8_t *end =
        find_crlfcrlf(data, len < OMT_HTTP_MAX_HEADER_BYTES ? len : OMT_HTTP_MAX_HEADER_BYTES);
    if (!end) return len >= OMT_HTTP_MAX_HEADER_BYTES ? -431 : 0;
    *head_len = (size_t)(end - data) + 4;
    const char *p = (const char *)data;
    const char *stop = (const char *)end;
    /* Request line. */
    const char *sp1 = memchr(p, ' ', (size_t)(stop - p));
    if (!sp1 || sp1 == p || (size_t)(sp1 - p) >= sizeof(req->method)) return -400;
    for (const char *c = p; c < sp1; c++)
        if (!is_tchar(*c)) return -400;
    memcpy(req->method, p, (size_t)(sp1 - p));
    req->method[sp1 - p] = 0;
    const char *line_end = memchr(sp1, '\r', (size_t)(stop - sp1));
    if (!line_end) line_end = stop;
    if (line_end[0] == '\r' && line_end + 1 < stop + 4 && line_end[1] != '\n') return -400;
    const char *sp2 = memchr(sp1 + 1, ' ', (size_t)(line_end - sp1 - 1));
    if (!sp2) return -400;
    const char *target = sp1 + 1;
    size_t target_len = (size_t)(sp2 - target);
    if (target_len == 0 || target[0] != '/') return -400;
    for (size_t i = 0; i < target_len; i++)
        if ((unsigned char)target[i] <= 0x20 || (unsigned char)target[i] >= 0x7F) return -400;
    const char *query = memchr(target, '?', target_len);
    size_t path_len = query ? (size_t)(query - target) : target_len;
    if (path_len >= sizeof(req->path)) return -414;
    memcpy(req->path, target, path_len);
    req->path[path_len] = 0;
    const char *version = sp2 + 1;
    size_t version_len = (size_t)(line_end - version);
    bool http11;
    if (version_len == 8 && memcmp(version, "HTTP/1.1", 8) == 0)
        http11 = true;
    else if (version_len == 8 && memcmp(version, "HTTP/1.0", 8) == 0)
        http11 = false;
    else
        return -505;
    /* Header fields. */
    req->header_count = 0;
    *content_length = 0;
    bool have_length = false, close = false, keep = false;
    const char *line = line_end + 2;
    while (line < stop) {
        const char *eol = line;
        while (eol < stop && *eol != '\r') eol++;
        if (eol < stop && (eol + 1 >= stop + 2 || eol[1] != '\n')) return -400;
        if (*line == ' ' || *line == '\t') return -400; /* obsolete line folding */
        const char *colon = memchr(line, ':', (size_t)(eol - line));
        if (!colon || colon == line) return -400;
        for (const char *c = line; c < colon; c++)
            if (!is_tchar(*c)) return -400;
        const char *value = colon + 1;
        while (value < eol && (*value == ' ' || *value == '\t')) value++;
        const char *vend = eol;
        while (vend > value && (vend[-1] == ' ' || vend[-1] == '\t')) vend--;
        for (const char *c = value; c < vend; c++)
            if (((unsigned char)*c < 0x20 && *c != '\t') || *c == 0x7F) return -400;
        if (req->header_count == OMT_HTTP_MAX_HEADERS) return -431;
        omt_http_header *h = &req->headers[req->header_count++];
        h->name = line;
        h->name_len = (size_t)(colon - line);
        h->value = value;
        h->value_len = (size_t)(vend - value);
        if (ieq(h->name, h->name_len, "content-length")) {
            uint64_t n;
            if (!omt_parse_u64(h->value, h->value_len, UINT32_MAX, &n)) return -400;
            if (have_length && n != *content_length) return -400;
            *content_length = (size_t)n;
            have_length = true;
        } else if (ieq(h->name, h->name_len, "transfer-encoding")) {
            return -411;
        } else if (ieq(h->name, h->name_len, "connection")) {
            /* A comma-separated token list. */
            const char *t = h->value, *tend = h->value + h->value_len;
            while (t < tend) {
                const char *comma = memchr(t, ',', (size_t)(tend - t));
                const char *e = comma ? comma : tend;
                const char *a = t, *b = e;
                while (a < b && (*a == ' ' || *a == '\t')) a++;
                while (b > a && (b[-1] == ' ' || b[-1] == '\t')) b--;
                if (ieq(a, (size_t)(b - a), "close")) close = true;
                if (ieq(a, (size_t)(b - a), "keep-alive")) keep = true;
                t = comma ? comma + 1 : tend;
            }
        }
        line = eol + 2;
    }
    *keep_alive = close ? false : (http11 || keep);
    return 1;
}

static void conn_free(conn *c) {
    if (!c) return;
    if (c->ssl) SSL_free(c->ssl);
    if (c->fd >= 0) close(c->fd);
    omt_buf_free(&c->in);
    omt_buf_free(&c->out);
    free(c);
}

static int alpn_select(SSL *ssl, const unsigned char **out, unsigned char *outlen,
                       const unsigned char *in, unsigned int inlen, void *arg) {
    (void)ssl;
    (void)arg;
    /* HTTP/1.1 only; a client offering nothing else proceeds without ALPN. */
    for (unsigned int i = 0; i < inlen;) {
        unsigned int n = in[i];
        if (i + 1 + n > inlen) break;
        if (n == 8 && memcmp(in + i + 1, "http/1.1", 8) == 0) {
            *out = in + i + 1;
            *outlen = (unsigned char)n;
            return SSL_TLSEXT_ERR_OK;
        }
        i += 1 + n;
    }
    return SSL_TLSEXT_ERR_NOACK;
}

static void openssl_error(omt_err *err, const char *what) {
    char text[256] = "unknown error";
    unsigned long code = ERR_get_error();
    if (code) ERR_error_string_n(code, text, sizeof(text));
    ERR_clear_error();
    omt_err_set(err, "%s: %s", what, text);
}

/* SSL_CTX_set1_groups_list is a macro that casts its string to char *. */
static bool set_groups(SSL_CTX *ctx) {
    char groups[] = "X25519:P-256:P-384";
    return SSL_CTX_set1_groups_list(ctx, groups) == 1;
}

bool omt_http_server_open(omt_http_server **out, uint16_t port, const char *cert_file,
                          const char *key_file, size_t max_body, omt_http_handler handler,
                          void *context, omt_err *err) {
    *out = NULL;
    omt_http_server *s = calloc(1, sizeof(*s));
    if (!s) {
        omt_err_set(err, "out of memory");
        return false;
    }
    s->listener = -1;
    s->max_body = max_body;
    s->handler = handler;
    s->context = context;
    s->ctx = SSL_CTX_new(TLS_server_method());
    if (!s->ctx) {
        openssl_error(err, "unable to create the TLS context");
        goto fail;
    }
    /* TLS 1.2 and 1.3 with forward-secret AEAD suites only -- the set the
     * rustls defaults offered. */
    SSL_CTX_set_min_proto_version(s->ctx, TLS1_2_VERSION);
    SSL_CTX_set_options(s->ctx, SSL_OP_NO_COMPRESSION | SSL_OP_NO_RENEGOTIATION |
                                    SSL_OP_CIPHER_SERVER_PREFERENCE | SSL_OP_NO_TICKET);
    SSL_CTX_set_mode(s->ctx, SSL_MODE_ENABLE_PARTIAL_WRITE | SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER);
    if (SSL_CTX_set_cipher_list(s->ctx,
                                "ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-ECDSA-AES128-GCM-SHA256:"
                                "ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-AES256-GCM-SHA384:"
                                "ECDHE-RSA-AES128-GCM-SHA256:ECDHE-RSA-CHACHA20-POLY1305") != 1 ||
        SSL_CTX_set_ciphersuites(s->ctx, "TLS_AES_256_GCM_SHA384:TLS_AES_128_GCM_SHA256:"
                                         "TLS_CHACHA20_POLY1305_SHA256") != 1 ||
        !set_groups(s->ctx)) {
        openssl_error(err, "unable to configure TLS");
        goto fail;
    }
    SSL_CTX_set_alpn_select_cb(s->ctx, alpn_select, NULL);
    if (SSL_CTX_use_certificate_chain_file(s->ctx, cert_file) != 1 ||
        SSL_CTX_use_PrivateKey_file(s->ctx, key_file, SSL_FILETYPE_PEM) != 1 ||
        SSL_CTX_check_private_key(s->ctx) != 1) {
        openssl_error(err, "unable to load TLS certificate");
        goto fail;
    }
    s->listener = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (s->listener < 0) {
        omt_err_os(err, errno);
        goto fail;
    }
    int one = 1;
    setsockopt(s->listener, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    struct sockaddr_in addr = {0};
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    if (bind(s->listener, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||
        listen(s->listener, 128) != 0) {
        omt_err_os(err, errno);
        goto fail;
    }
    *out = s;
    return true;
fail:
    omt_http_server_close(s);
    return false;
}

void omt_http_server_close(omt_http_server *s) {
    if (!s) return;
    for (size_t i = 0; i < OMT_HTTP_MAX_CONNECTIONS; i++) conn_free(s->conns[i]);
    if (s->listener >= 0) close(s->listener);
    if (s->ctx) SSL_CTX_free(s->ctx);
    free(s);
}

static void http_date(char out[40]) {
    static const char *const days[] = {"Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"};
    static const char *const months[] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                         "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};
    int64_t secs;
    uint32_t nanos;
    omt_wall_clock(&secs, &nanos);
    int64_t day_number = secs / 86400;
    int64_t rest = secs % 86400;
    int64_t year;
    unsigned month, day;
    omt_civil_from_days(day_number, &year, &month, &day);
    snprintf(out, 40, "%s, %02u %s %04lld %02u:%02u:%02u GMT", days[((day_number % 7) + 7) % 7],
             day, months[month - 1], (long long)year, (unsigned)(rest / 3600),
             (unsigned)(rest % 3600 / 60), (unsigned)(rest % 60));
}

/* Serialises a response into the connection's output buffer. */
static void queue_response(conn *c, omt_http_response *res) {
    char date[40];
    http_date(date);
    omt_buf_clear(&c->out);
    c->out_pos = 0;
    if (res->headers.failed || res->body.failed) {
        res->status = 500;
        omt_buf_clear(&res->headers);
        omt_buf_clear(&res->body);
    }
    omt_buf_printf(&c->out, "HTTP/1.1 %d %s\r\n", res->status, omt_http_reason(res->status));
    omt_buf_append(&c->out, res->headers.data ? res->headers.data : (const uint8_t *)"",
                   res->headers.len);
    omt_buf_printf(&c->out, "content-length: %zu\r\ndate: %s\r\n", res->body.len, date);
    if (c->close_after) omt_buf_puts(&c->out, "connection: close\r\n");
    omt_buf_puts(&c->out, "\r\n");
    if (!res->head)
        omt_buf_append(&c->out, res->body.data ? res->body.data : (const uint8_t *)"",
                       res->body.len);
    c->state = CONN_WRITE;
    c->deadline = omt_now_ms() + OMT_HTTP_WRITE_TIMEOUT_MS;
}

static void response_init(omt_http_response *res, size_t body_limit) {
    memset(res, 0, sizeof(*res));
    omt_buf_init(&res->headers, 64 * 1024);
    omt_buf_init(&res->body, body_limit);
}

static void response_free(omt_http_response *res) {
    omt_buf_free(&res->headers);
    omt_buf_free(&res->body);
}

/* A protocol-level refusal: no handler involved, and the connection closes. */
static void refuse(conn *c, int status) {
    omt_http_response res;
    response_init(&res, 1024);
    res.status = status;
    c->close_after = true;
    queue_response(c, &res);
    response_free(&res);
}

static void dispatch(omt_http_server *s, conn *c) {
    c->req.body = c->in.data + c->head_len;
    c->req.body_len = c->content_length;
    omt_strlcpy(c->req.peer, c->peer, sizeof(c->req.peer));
    omt_http_response res;
    /* The support bundle is the largest response: 8 MiB of capture plus its
     * text members, inside the ZIP's own ceiling. */
    response_init(&res, 64u * 1024 * 1024);
    res.status = 200;
    res.head = strcmp(c->req.method, "HEAD") == 0;
    s->handler(s->context, &c->req, &res);
    c->close_after = !c->keep_alive;
    queue_response(c, &res);
    response_free(&res);
}

/* Tries to parse what has arrived; dispatches a complete request. */
static void process_input(omt_http_server *s, conn *c) {
    if (!c->head_parsed) {
        int r = omt_http_parse_head(c->in.data, c->in.len, &c->req, &c->head_len,
                                    &c->content_length, &c->keep_alive);
        if (r == 0) return;
        if (r < 0) {
            refuse(c, -r);
            return;
        }
        c->head_parsed = true;
        if (c->content_length > s->max_body) {
            refuse(c, 413);
            return;
        }
        c->deadline = omt_now_ms() + OMT_HTTP_BODY_TIMEOUT_MS;
        char expect[32];
        if (c->content_length && !c->sent_continue &&
            omt_http_header_value(&c->req, "expect", expect, sizeof(expect)) &&
            omt_ascii_ieq(expect, strlen(expect), "100-continue") &&
            c->in.len < c->head_len + c->content_length) {
            /* Interim response, written directly: it is small and the socket
             * has just delivered data, so a short write is not expected; a
             * failure only costs the client its 1 s wait. */
            static const char cont[] = "HTTP/1.1 100 Continue\r\n\r\n";
            (void)SSL_write(c->ssl, cont, (int)sizeof(cont) - 1);
            c->sent_continue = true;
        }
    }
    if (c->in.len - c->head_len < c->content_length) return;
    dispatch(s, c);
}

static void after_write(conn *c) {
    if (c->close_after) {
        c->state = CONN_SHUTDOWN;
        return;
    }
    /* Keep the connection, and any pipelined bytes after this request. */
    size_t used = c->head_len + c->content_length;
    omt_buf_consume(&c->in, used);
    c->head_parsed = false;
    c->head_len = c->content_length = 0;
    c->sent_continue = false;
    c->state = CONN_READ;
    c->deadline =
        omt_now_ms() + (c->in.len ? OMT_HTTP_HEADER_TIMEOUT_MS : OMT_HTTP_IDLE_TIMEOUT_MS);
}

/* Drives one connection as far as it will go without blocking. Returns false
 * when the connection is finished and should be freed. */
static bool drive(omt_http_server *s, conn *c) {
    for (;;) {
        ERR_clear_error();
        if (c->state == CONN_HANDSHAKE) {
            int rc = SSL_do_handshake(c->ssl);
            if (rc == 1) {
                c->state = CONN_READ;
                c->deadline = omt_now_ms() + OMT_HTTP_HEADER_TIMEOUT_MS;
                continue;
            }
            int e = SSL_get_error(c->ssl, rc);
            if (e == SSL_ERROR_WANT_READ)
                c->want = POLLIN;
            else if (e == SSL_ERROR_WANT_WRITE)
                c->want = POLLOUT;
            else
                return false;
            return true;
        }
        if (c->state == CONN_READ) {
            uint8_t chunk[16384];
            int rc = SSL_read(c->ssl, chunk, sizeof(chunk));
            if (rc > 0) {
                if (c->in.len == 0 && !c->head_parsed)
                    c->deadline = omt_now_ms() + OMT_HTTP_HEADER_TIMEOUT_MS;
                omt_buf_append(&c->in, chunk, (size_t)rc);
                if (c->in.failed) {
                    refuse(c, 413);
                    continue;
                }
                process_input(s, c);
                continue;
            }
            int e = SSL_get_error(c->ssl, rc);
            if (e == SSL_ERROR_WANT_READ) {
                c->want = POLLIN;
                return true;
            }
            if (e == SSL_ERROR_WANT_WRITE) {
                c->want = POLLOUT;
                return true;
            }
            return false; /* closed by the peer or failed */
        }
        if (c->state == CONN_WRITE) {
            if (c->out_pos == c->out.len) {
                after_write(c);
                if (c->state == CONN_READ && c->in.len) process_input(s, c);
                continue;
            }
            size_t left = c->out.len - c->out_pos;
            int rc = SSL_write(c->ssl, c->out.data + c->out_pos, (int)omt_min_size(left, 1u << 20));
            if (rc > 0) {
                c->out_pos += (size_t)rc;
                continue;
            }
            int e = SSL_get_error(c->ssl, rc);
            if (e == SSL_ERROR_WANT_READ)
                c->want = POLLIN;
            else if (e == SSL_ERROR_WANT_WRITE)
                c->want = POLLOUT;
            else
                return false;
            return true;
        }
        /* CONN_SHUTDOWN: a best-effort close_notify, then done. */
        (void)SSL_shutdown(c->ssl);
        return false;
    }
}

static void accept_one(omt_http_server *s) {
    struct sockaddr_storage peer;
    socklen_t peer_len = sizeof(peer);
    int fd =
        accept4(s->listener, (struct sockaddr *)&peer, &peer_len, SOCK_CLOEXEC | SOCK_NONBLOCK);
    if (fd < 0) return;
    size_t slot = OMT_HTTP_MAX_CONNECTIONS;
    for (size_t i = 0; i < OMT_HTTP_MAX_CONNECTIONS; i++)
        if (!s->conns[i]) {
            slot = i;
            break;
        }
    if (slot == OMT_HTTP_MAX_CONNECTIONS) {
        /* Over the connection ceiling: refused at the door. */
        close(fd);
        return;
    }
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
    conn *c = calloc(1, sizeof(*c));
    if (!c) {
        close(fd);
        return;
    }
    c->fd = fd;
    c->ssl = SSL_new(s->ctx);
    if (!c->ssl || SSL_set_fd(c->ssl, fd) != 1) {
        conn_free(c);
        return;
    }
    SSL_set_accept_state(c->ssl);
    omt_buf_init(&c->in, OMT_HTTP_MAX_HEADER_BYTES + s->max_body + 16384);
    omt_buf_init(&c->out, 80u * 1024 * 1024);
    c->state = CONN_HANDSHAKE;
    c->want = POLLIN;
    c->deadline = omt_now_ms() + OMT_HTTP_HEADER_TIMEOUT_MS;
    if (peer.ss_family == AF_INET)
        inet_ntop(AF_INET, &((struct sockaddr_in *)&peer)->sin_addr, c->peer, sizeof(c->peer));
    else
        omt_strlcpy(c->peer, "unknown", sizeof(c->peer));
    s->conns[slot] = c;
    if (!drive(s, c)) {
        conn_free(c);
        s->conns[slot] = NULL;
    }
}

bool omt_http_server_run(omt_http_server *s, volatile int *stop, omt_err *err) {
    struct pollfd fds[OMT_HTTP_MAX_CONNECTIONS + 1];
    size_t index[OMT_HTTP_MAX_CONNECTIONS + 1];
    while (!*stop) {
        nfds_t n = 0;
        fds[n++] = (struct pollfd){s->listener, POLLIN, 0};
        uint64_t now = omt_now_ms();
        uint64_t wait = 1000;
        for (size_t i = 0; i < OMT_HTTP_MAX_CONNECTIONS; i++) {
            conn *c = s->conns[i];
            if (!c) continue;
            if (c->deadline <= now) {
                conn_free(c);
                s->conns[i] = NULL;
                continue;
            }
            if (c->deadline - now < wait) wait = c->deadline - now;
            index[n] = i;
            fds[n++] = (struct pollfd){c->fd, c->want, 0};
        }
        int rc = poll(fds, n, (int)wait);
        if (rc < 0) {
            if (errno == EINTR) continue;
            omt_err_os(err, errno);
            return false;
        }
        for (nfds_t k = 1; k < n; k++) {
            if (!fds[k].revents) continue;
            conn *c = s->conns[index[k]];
            if (!c) continue;
            if (!drive(s, c)) {
                conn_free(c);
                s->conns[index[k]] = NULL;
            }
        }
        if (fds[0].revents & POLLIN)
            for (int burst = 0; burst < 16; burst++) {
                int before = 0;
                for (size_t i = 0; i < OMT_HTTP_MAX_CONNECTIONS; i++) before += s->conns[i] != NULL;
                accept_one(s);
                int after = 0;
                for (size_t i = 0; i < OMT_HTTP_MAX_CONNECTIONS; i++) after += s->conns[i] != NULL;
                if (after == before) break;
            }
    }
    return true;
}
