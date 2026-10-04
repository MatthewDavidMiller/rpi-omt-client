/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The cryptographic primitives the Web frontend and the deployer use, all from
 * OpenSSL's libcrypto: the one third-party library the project links. Nothing
 * here implements a primitive; it only adapts OpenSSL's EVP interfaces to the
 * project's bounded buffers and error type.
 */
#ifndef OMT_CRYPTO_H
#define OMT_CRYPTO_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

#define OMT_SHA256_LEN 32
#define OMT_SHA512_LEN 64

OMT_NODISCARD bool omt_sha256(const void *data, size_t len, uint8_t out[OMT_SHA256_LEN]);
OMT_NODISCARD bool omt_sha512(const void *data, size_t len, uint8_t out[OMT_SHA512_LEN]);
/* Appends the lowercase hex SHA-256 of data. */
OMT_NODISCARD bool omt_sha256_hex(const void *data, size_t len, omt_buf *out);
OMT_NODISCARD bool omt_hmac_sha256(const void *key, size_t key_len, const void *data, size_t len,
                                   uint8_t out[OMT_SHA256_LEN]);
OMT_NODISCARD bool omt_hmac_sha256_hex(const void *key, size_t key_len, const void *data,
                                       size_t len, omt_buf *out);
OMT_NODISCARD bool omt_pbkdf2_sha256(const void *password, size_t password_len, const void *salt,
                                     size_t salt_len, uint32_t iterations, uint8_t *out,
                                     size_t out_len);
OMT_NODISCARD bool omt_pbkdf2_sha1(const void *password, size_t password_len, const void *salt,
                                   size_t salt_len, uint32_t iterations, uint8_t *out,
                                   size_t out_len);
/* scrypt with N = 2^log_n. Parameters are checked against the bounds the Rust
 * scrypt crate enforced before any work is done. */
OMT_NODISCARD bool omt_scrypt(const void *password, size_t password_len, const void *salt,
                              size_t salt_len, uint64_t n, uint32_t r, uint32_t p, uint8_t *out,
                              size_t out_len);
/* Constant-time comparison; lengths are compared first, as the Rust code did. */
bool omt_ct_equal(const void *a, size_t a_len, const void *b, size_t b_len);
/* Wipes memory in a way the compiler cannot elide. */
void omt_cleanse(void *p, size_t len);

/* Incremental SHA-256 for streams too large to buffer. */
typedef struct omt_sha256_ctx omt_sha256_ctx;
omt_sha256_ctx *omt_sha256_begin(void);
OMT_NODISCARD bool omt_sha256_update(omt_sha256_ctx *c, const void *data, size_t len);
OMT_NODISCARD bool omt_sha256_finish(omt_sha256_ctx *c, uint8_t out[OMT_SHA256_LEN]);

#endif
