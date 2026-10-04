/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Trust roots on Windows: the machine's ROOT certificate store, the same set
 * Schannel and the Rust deployer's native-certs loader trust.
 */
/* windows.h first: wincrypt.h uses its types. */
#include <windows.h>

#include <wincrypt.h>

#include <openssl/x509.h>

#include "deploy/core/https.h"

bool dp_tls_load_roots(SSL_CTX *ctx, dp_err *err) {
    HCERTSTORE store = CertOpenSystemStoreW(0, L"ROOT");
    if (!store) {
        dp_fail_os(err, "cannot open the Windows ROOT certificate store");
        return false;
    }
    X509_STORE *trust = SSL_CTX_get_cert_store(ctx);
    size_t loaded = 0;
    PCCERT_CONTEXT cert = NULL;
    while ((cert = CertEnumCertificatesInStore(store, cert)) != NULL) {
        const unsigned char *der = cert->pbCertEncoded;
        X509 *x = d2i_X509(NULL, &der, (long)cert->cbCertEncoded);
        if (!x) continue;
        if (X509_STORE_add_cert(trust, x) == 1) loaded++;
        X509_free(x);
    }
    CertCloseStore(store, 0);
    if (loaded == 0) {
        dp_fail(err, "no native TLS root certificates were available");
        return false;
    }
    return true;
}
