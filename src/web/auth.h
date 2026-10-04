/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Web GUI authentication: the operator password, the persistent session
 * registry, and CSRF tokens.
 *
 * Sessions are random 256-bit identifiers; the registry on disk stores only
 * HMAC(secret, id), so reading it does not yield a usable cookie. Each record
 * also carries HMAC(secret, password), which is how changing the password
 * ends every existing session. CSRF tokens are HMAC(secret, scope, nonce) and
 * are compared in constant time.
 */
#ifndef OMT_WEB_AUTH_H
#define OMT_WEB_AUTH_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"
#include "web/settings.h"

#define OMT_MINIMUM_PASSWORD_BYTES 12
#define OMT_MAXIMUM_PASSWORD_BYTES 128
#define OMT_SESSION_ID_HEX 64

typedef enum { PASSWORD_PLAIN, PASSWORD_PBKDF2, PASSWORD_SCRYPT } omt_password_kind;

typedef struct {
    omt_password_kind kind;
    char *plain;
    size_t plain_len;
    uint32_t iterations;
    uint64_t n;
    uint32_t r, p;
    uint8_t *salt;
    size_t salt_len;
    uint8_t digest[64];
    size_t digest_len;
} omt_password;

/* Parses a stored password: Werkzeug PBKDF2-SHA256 or scrypt hashes, or a
 * plaintext value. Any other pbkdf2:/argon2: prefix is refused. */
OMT_NODISCARD bool omt_password_parse(const char *value, size_t len, omt_password *out,
                                      omt_err *err);
bool omt_password_verify(const omt_password *p, const char *supplied, size_t len);
void omt_password_free(omt_password *p);

typedef struct {
    const omt_web_settings *settings;
    uint8_t secret[256];
    size_t secret_len;
    omt_password password;
    char password_digest[OMT_SESSION_ID_HEX + 1];
} omt_auth;

OMT_NODISCARD bool omt_auth_load(omt_auth *a, const omt_web_settings *s, omt_err *err);
void omt_auth_free(omt_auth *a);
/* On a correct password writes a fresh session id (64 hex chars) into
 * `session_out` and returns 1; returns 0 for a wrong password and -1 when the
 * registry could not be written. `previous` is revoked when given. */
int omt_auth_authenticate(omt_auth *a, const char *password, size_t len, const char *previous,
                          char session_out[OMT_SESSION_ID_HEX + 1], omt_err *err);
bool omt_auth_is_current(omt_auth *a, const char *session_id);
OMT_NODISCARD bool omt_auth_revoke(omt_auth *a, const char *session_id, omt_err *err);
OMT_NODISCARD bool omt_auth_csrf_token(const omt_auth *a, const char *scope, const char *nonce,
                                       char out[OMT_SESSION_ID_HEX + 1]);
bool omt_auth_verify_csrf(const omt_auth *a, const char *scope, const char *nonce,
                          const char *token);

OMT_NODISCARD bool omt_validate_new_password(const char *password, size_t len, omt_err *err);
/* Writes "pbkdf2:sha256:600000$SALT$DIGEST\n" for a new password. */
OMT_NODISCARD bool omt_encode_password(const char *password, size_t len, omt_buf *out,
                                       omt_err *err);
OMT_NODISCARD bool omt_replace_password(const omt_web_settings *s, const char *password, size_t len,
                                        omt_err *err);
/* Creates the Web secret and, when no password is set, a random one, which is
 * written to `generated` (empty when a password already existed). */
OMT_NODISCARD bool omt_auth_initialize(const omt_web_settings *s, omt_buf *generated, omt_err *err);
void omt_remove_legacy_secret(const omt_web_settings *s);

#endif
