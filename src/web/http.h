/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * A small HTTPS/1.1 server: one process, one poll() loop, non-blocking
 * OpenSSL connections, and a synchronous handler called once a request is
 * complete. The handler serialisation matches the Rust frontend, which ran its
 * blocking handlers on a single-threaded runtime.
 *
 * Unlike that frontend, every connection is bounded in time and number: a
 * client gets OMT_HTTP_HEADER_TIMEOUT_MS to send its headers, the body
 * timeout to send its body, and an idle keep-alive connection is closed after
 * the idle timeout, so a slow or silent client cannot hold a slot forever.
 */
#ifndef OMT_WEB_HTTP_H
#define OMT_WEB_HTTP_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

#define OMT_HTTP_MAX_CONNECTIONS 64
#define OMT_HTTP_MAX_HEADER_BYTES (16 * 1024)
#define OMT_HTTP_MAX_HEADERS 64
#define OMT_HTTP_HEADER_TIMEOUT_MS 15000u
#define OMT_HTTP_BODY_TIMEOUT_MS 30000u
#define OMT_HTTP_IDLE_TIMEOUT_MS 60000u
#define OMT_HTTP_WRITE_TIMEOUT_MS 120000u

typedef struct {
    const char *name;
    size_t name_len;
    const char *value;
    size_t value_len;
} omt_http_header;

typedef struct {
    char method[16];
    char path[2048];
    omt_http_header headers[OMT_HTTP_MAX_HEADERS];
    size_t header_count;
    const uint8_t *body;
    size_t body_len;
    char peer[64]; /* the client address, for rate limiting */
} omt_http_request;

typedef struct {
    int status;
    omt_buf headers; /* "name: value\r\n" lines */
    omt_buf body;
    bool head; /* send headers only */
} omt_http_response;

/* The value of the first header named `name` (case-insensitive), as a
 * NUL-terminated copy in `out`. */
bool omt_http_header_value(const omt_http_request *req, const char *name, char *out, size_t size);
void omt_http_add_header(omt_http_response *res, const char *name, const char *value);
const char *omt_http_reason(int status);

typedef void (*omt_http_handler)(void *context, const omt_http_request *req,
                                 omt_http_response *res);

typedef struct omt_http_server omt_http_server;

/* Loads the PEM certificate chain and key and binds 0.0.0.0:port. */
OMT_NODISCARD bool omt_http_server_open(omt_http_server **out, uint16_t port, const char *cert_file,
                                        const char *key_file, size_t max_body,
                                        omt_http_handler handler, void *context, omt_err *err);
/* Serves until a fatal error or *stop becomes true. */
OMT_NODISCARD bool omt_http_server_run(omt_http_server *s, volatile int *stop, omt_err *err);
void omt_http_server_close(omt_http_server *s);

/* Parses one request head (exposed for tests and the fuzzer). Returns 1 when
 * complete, 0 when more bytes are needed, and a negative HTTP status for a
 * request that must be refused. On success `head_len` is the header block
 * size and `content_length` the declared body. */
int omt_http_parse_head(const uint8_t *data, size_t len, omt_http_request *req, size_t *head_len,
                        size_t *content_length, bool *keep_alive);

#endif
