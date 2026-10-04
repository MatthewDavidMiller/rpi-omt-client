/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The deployer's one HTTPS client, and the platform's trust roots for it.
 */
#ifndef DP_HTTPS_H
#define DP_HTTPS_H

#include <openssl/ssl.h>

#include "common/buf.h"
#include "deploy/core/sys.h"

/* GETs url into body, following at most five HTTPS redirects, refusing a body
 * over `limit` bytes, and finishing within `timeout_ms` overall. */
bool dp_https_get(const char *url, size_t limit, uint32_t timeout_ms, const dp_cancel *cancel,
                  omt_buf *body, dp_err *err);
/* Loads the operating system's trust roots into ctx. */
bool dp_tls_load_roots(SSL_CTX *ctx, dp_err *err);

#endif
