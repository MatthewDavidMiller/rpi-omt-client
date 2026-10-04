/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Trust roots on Linux. A static binary cannot rely on the OpenSSL directory
 * it was configured with -- that is the build host's layout -- so this probes
 * the bundle locations the distributions use, as rustls-native-certs did:
 * SSL_CERT_FILE and SSL_CERT_DIR first, then the well-known paths.
 */
#include <openssl/x509_vfy.h>

#include "deploy/core/https.h"

static const char *const BUNDLES[] = {
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/share/certs/ca-root-nss.crt",
    "/etc/pki/tls/cacert.pem",
    "/etc/certs/ca-certificates.crt",
};

static const char *const DIRECTORIES[] = {
    "/etc/ssl/certs",
    "/etc/pki/tls/certs",
};

bool dp_tls_load_roots(SSL_CTX *ctx, dp_err *err) {
    omt_buf value;
    omt_buf_init(&value, 8192);
    bool loaded = false;
    if (dp_getenv("SSL_CERT_FILE", &value) && value.len) {
        loaded = SSL_CTX_load_verify_locations(ctx, omt_buf_cstr(&value), NULL) == 1;
    }
    omt_buf_clear(&value);
    if (dp_getenv("SSL_CERT_DIR", &value) && value.len && dp_is_dir(omt_buf_cstr(&value))) {
        loaded |= SSL_CTX_load_verify_locations(ctx, NULL, omt_buf_cstr(&value)) == 1;
    }
    omt_buf_free(&value);
    for (size_t i = 0; !loaded && i < OMT_ARRAY_LEN(BUNDLES); i++) {
        if (dp_is_file(BUNDLES[i])) {
            loaded = SSL_CTX_load_verify_locations(ctx, BUNDLES[i], NULL) == 1;
        }
    }
    for (size_t i = 0; !loaded && i < OMT_ARRAY_LEN(DIRECTORIES); i++) {
        if (dp_is_dir(DIRECTORIES[i])) {
            loaded = SSL_CTX_load_verify_locations(ctx, NULL, DIRECTORIES[i]) == 1;
        }
    }
    if (!loaded) {
        dp_fail(err, "no native TLS root certificates were available");
        return false;
    }
    return true;
}
