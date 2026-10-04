/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "crypto/crypto.h"

#include <openssl/crypto.h>
#include <openssl/evp.h>
#include <openssl/hmac.h>

bool omt_sha256(const void *data, size_t len, uint8_t out[OMT_SHA256_LEN]) {
    unsigned int n = 0;
    return EVP_Digest(data, len, out, &n, EVP_sha256(), NULL) == 1 && n == OMT_SHA256_LEN;
}

bool omt_sha512(const void *data, size_t len, uint8_t out[OMT_SHA512_LEN]) {
    unsigned int n = 0;
    return EVP_Digest(data, len, out, &n, EVP_sha512(), NULL) == 1 && n == OMT_SHA512_LEN;
}

bool omt_sha256_hex(const void *data, size_t len, omt_buf *out) {
    uint8_t digest[OMT_SHA256_LEN];
    if (!omt_sha256(data, len, digest)) return false;
    omt_buf_hex(out, digest, sizeof(digest));
    return !out->failed;
}

bool omt_hmac_sha256(const void *key, size_t key_len, const void *data, size_t len,
                     uint8_t out[OMT_SHA256_LEN]) {
    unsigned int n = 0;
    if (key_len > INT32_MAX) return false;
    return HMAC(EVP_sha256(), key, (int)key_len, data, len, out, &n) != NULL && n == OMT_SHA256_LEN;
}

bool omt_hmac_sha256_hex(const void *key, size_t key_len, const void *data, size_t len,
                         omt_buf *out) {
    uint8_t mac[OMT_SHA256_LEN];
    if (!omt_hmac_sha256(key, key_len, data, len, mac)) return false;
    omt_buf_hex(out, mac, sizeof(mac));
    return !out->failed;
}

static bool pbkdf2(const EVP_MD *md, const void *password, size_t password_len, const void *salt,
                   size_t salt_len, uint32_t iterations, uint8_t *out, size_t out_len) {
    if (password_len > INT32_MAX || salt_len > INT32_MAX || out_len > INT32_MAX ||
        iterations == 0 || iterations > INT32_MAX)
        return false;
    return PKCS5_PBKDF2_HMAC(password, (int)password_len, salt, (int)salt_len, (int)iterations, md,
                             (int)out_len, out) == 1;
}

bool omt_pbkdf2_sha256(const void *password, size_t password_len, const void *salt, size_t salt_len,
                       uint32_t iterations, uint8_t *out, size_t out_len) {
    return pbkdf2(EVP_sha256(), password, password_len, salt, salt_len, iterations, out, out_len);
}

bool omt_pbkdf2_sha1(const void *password, size_t password_len, const void *salt, size_t salt_len,
                     uint32_t iterations, uint8_t *out, size_t out_len) {
    return pbkdf2(EVP_sha1(), password, password_len, salt, salt_len, iterations, out, out_len);
}

bool omt_scrypt(const void *password, size_t password_len, const void *salt, size_t salt_len,
                uint64_t n, uint32_t r, uint32_t p, uint8_t *out, size_t out_len) {
    /* The bounds of the Rust scrypt crate's Params::new: log2(N) < 64,
     * r and p non-zero, r*p < 2^30, and an output of 10-64 bytes. */
    if (n < 2 || (n & (n - 1)) != 0 || r == 0 || p == 0 ||
        (uint64_t)r * (uint64_t)p >= (1u << 30) || out_len < 10 || out_len > 64)
        return false;
    /* 128 * r * N bytes of working memory, which OpenSSL also bounds. Allow
     * up to 1 GiB so the Werkzeug default (N=32768, r=8: 32 MiB) fits. */
    uint64_t memory = 128ull * r * n * 2 + 128ull * r * p;
    if (memory > (1ull << 30)) return false;
    return EVP_PBE_scrypt(password, password_len, salt, salt_len, n, r, p, memory, out, out_len) ==
           1;
}

bool omt_ct_equal(const void *a, size_t a_len, const void *b, size_t b_len) {
    return a_len == b_len && (a_len == 0 || CRYPTO_memcmp(a, b, a_len) == 0);
}

void omt_cleanse(void *p, size_t len) {
    if (p && len) OPENSSL_cleanse(p, len);
}

struct omt_sha256_ctx {
    EVP_MD_CTX *md;
};

omt_sha256_ctx *omt_sha256_begin(void) {
    omt_sha256_ctx *c = OPENSSL_zalloc(sizeof(*c));
    if (!c) return NULL;
    c->md = EVP_MD_CTX_new();
    if (!c->md || EVP_DigestInit_ex(c->md, EVP_sha256(), NULL) != 1) {
        EVP_MD_CTX_free(c->md);
        OPENSSL_free(c);
        return NULL;
    }
    return c;
}

bool omt_sha256_update(omt_sha256_ctx *c, const void *data, size_t len) {
    return EVP_DigestUpdate(c->md, data, len) == 1;
}

bool omt_sha256_finish(omt_sha256_ctx *c, uint8_t out[OMT_SHA256_LEN]) {
    unsigned int n = 0;
    bool ok = EVP_DigestFinal_ex(c->md, out, &n) == 1 && n == OMT_SHA256_LEN;
    EVP_MD_CTX_free(c->md);
    OPENSSL_free(c);
    return ok;
}
