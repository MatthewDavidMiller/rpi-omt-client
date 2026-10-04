/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * User authentication (RFC 4252, 4256, and RFC 8332's RSA signature
 * selection). Factory Alpine images accept root with an empty password, and
 * OpenSSH and Dropbear advertise the methods that allow it in different
 * orders, so a password connection tries `none` (for an empty password),
 * then `password`, then `keyboard-interactive` -- the order the Rust client
 * used.
 */
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

#define KBD_ROUNDS 8

typedef enum { AUTH_SUCCESS, AUTH_FAILURE, AUTH_ERROR } auth_status;

static void request_head(omt_buf *b, const char *user, const char *method) {
    sb_u8(b, SSH_MSG_USERAUTH_REQUEST);
    sb_cstring(b, user);
    sb_cstring(b, "ssh-connection");
    sb_cstring(b, method);
}

/* The next reply that is not a banner. */
static bool next_reply(ssh_transport *t, dp_err *err) {
    for (;;) {
        if (!ssh_recv_wait(t, err)) return false;
        if (t->payload.data[0] != SSH_MSG_USERAUTH_BANNER) return true;
    }
}

static auth_status verdict(ssh_transport *t, dp_err *err) {
    switch (t->payload.data[0]) {
    case SSH_MSG_USERAUTH_SUCCESS: return AUTH_SUCCESS;
    case SSH_MSG_USERAUTH_FAILURE: return AUTH_FAILURE;
    default:
        dp_fail(err, "unexpected SSH message %u during authentication",
                (unsigned)t->payload.data[0]);
        return AUTH_ERROR;
    }
}

static auth_status send_and_wait(ssh_transport *t, omt_buf *msg, dp_err *err) {
    bool sent = ssh_send(t, msg, err);
    omt_buf_free_secret(msg);
    if (!sent || !next_reply(t, err)) return AUTH_ERROR;
    return verdict(t, err);
}

static auth_status try_none(ssh_transport *t, const char *user, dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 1024);
    request_head(&msg, user, "none");
    return send_and_wait(t, &msg, err);
}

static auth_status try_password(ssh_transport *t, const char *user, const dp_secret *password,
                                dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 8192);
    request_head(&msg, user, "password");
    sb_bool(&msg, false);
    sb_string(&msg, dp_secret_text(password), dp_secret_len(password));
    bool sent = ssh_send(t, &msg, err);
    omt_buf_free_secret(&msg);
    if (!sent || !next_reply(t, err)) return AUTH_ERROR;
    /* PASSWD_CHANGEREQ: an expired password is a failure here. */
    if (t->payload.data[0] == SSH_MSG_USERAUTH_60) return AUTH_FAILURE;
    return verdict(t, err);
}

static auth_status try_keyboard(ssh_transport *t, const char *user, const dp_secret *password,
                                dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 1024);
    request_head(&msg, user, "keyboard-interactive");
    sb_cstring(&msg, "");
    sb_cstring(&msg, "");
    bool sent = ssh_send(t, &msg, err);
    omt_buf_free(&msg);
    if (!sent) return AUTH_ERROR;
    for (int round = 0; round < KBD_ROUNDS; round++) {
        if (!next_reply(t, err)) return AUTH_ERROR;
        if (t->payload.data[0] != SSH_MSG_USERAUTH_60) return verdict(t, err);
        /* INFO_REQUEST: answer every prompt with the password. */
        omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1), skip;
        uint32_t prompts;
        if (!sr_string(&r, &skip) || !sr_string(&r, &skip) || !sr_string(&r, &skip) ||
            !sr_u32(&r, &prompts) || prompts > 64) {
            dp_fail(err, "malformed keyboard-interactive request");
            return AUTH_ERROR;
        }
        omt_buf reply;
        omt_buf_init(&reply, 64u * 1024u);
        sb_u8(&reply, SSH_MSG_USERAUTH_INFO_RESPONSE);
        sb_u32(&reply, prompts);
        for (uint32_t i = 0; i < prompts; i++) {
            bool echo;
            if (!sr_string(&r, &skip) || !sr_bool(&r, &echo)) {
                omt_buf_free_secret(&reply);
                dp_fail(err, "malformed keyboard-interactive request");
                return AUTH_ERROR;
            }
            sb_string(&reply, dp_secret_text(password), dp_secret_len(password));
        }
        sent = ssh_send(t, &reply, err);
        omt_buf_free_secret(&reply);
        if (!sent) return AUTH_ERROR;
    }
    return AUTH_FAILURE;
}

/* rsa-sha2-512 unless the server's EXT_INFO lists only rsa-sha2-256. */
static const char *signature_alg(const ssh_transport *t, const ssh_privkey *key) {
    if (strcmp(key->type, "ssh-rsa") != 0) return key->type;
    omt_span algs = omt_span_of(t->server_sig_algs.data, t->server_sig_algs.len);
    if (algs.len && !namelist_has(algs, "rsa-sha2-512") && namelist_has(algs, "rsa-sha2-256")) {
        return "rsa-sha2-256";
    }
    return "rsa-sha2-512";
}

static auth_status try_publickey(ssh_transport *t, const char *user, const ssh_privkey *key,
                                 dp_err *err) {
    const char *alg = signature_alg(t, key);
    omt_buf signed_data, msg, sig;
    omt_buf_init(&signed_data, 64u * 1024u);
    omt_buf_init(&msg, 64u * 1024u);
    omt_buf_init(&sig, 8192);
    sb_string(&signed_data, t->session_id.data, t->session_id.len);
    request_head(&signed_data, user, "publickey");
    sb_bool(&signed_data, true);
    sb_cstring(&signed_data, alg);
    sb_string(&signed_data, key->blob.data, key->blob.len);
    auth_status status = AUTH_ERROR;
    if (!signed_data.failed &&
        ssh_privkey_sign(key, alg, signed_data.data, signed_data.len, &sig, err)) {
        /* The request is the signed data without its session-id prefix. */
        size_t prefix = 4 + t->session_id.len;
        omt_buf_append(&msg, signed_data.data + prefix, signed_data.len - prefix);
        sb_string(&msg, sig.data, sig.len);
        status = send_and_wait(t, &msg, err);
    }
    omt_buf_free(&signed_data);
    omt_buf_free(&msg);
    omt_buf_free(&sig);
    return status;
}

bool ssh_authenticate(ssh_transport *t, const dp_connection *c, dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 64);
    sb_u8(&msg, SSH_MSG_SERVICE_REQUEST);
    sb_cstring(&msg, "ssh-userauth");
    bool ok = ssh_send(t, &msg, err) && ssh_recv_wait(t, err);
    omt_buf_free(&msg);
    if (!ok) return false;
    if (t->payload.data[0] != SSH_MSG_SERVICE_ACCEPT) {
        dp_fail(err, "the SSH server refused user authentication");
        return false;
    }
    auth_status status = AUTH_FAILURE;
    if (c->auth == DP_AUTH_PASSWORD) {
        if (!c->password.set) {
            dp_fail(err, "SSH password is missing");
            return false;
        }
        if (dp_secret_len(&c->password) == 0) status = try_none(t, c->username, err);
        if (status == AUTH_FAILURE) status = try_password(t, c->username, &c->password, err);
        if (status == AUTH_FAILURE) status = try_keyboard(t, c->username, &c->password, err);
    } else {
        if (!c->key_path) {
            dp_fail(err, "SSH private-key path is missing");
            return false;
        }
        ssh_privkey key;
        if (!ssh_privkey_load(c->key_path, dp_secret_text(&c->key_passphrase),
                              dp_secret_len(&c->key_passphrase), &key, err)) {
            return false;
        }
        status = try_publickey(t, c->username, &key, err);
        ssh_privkey_free(&key);
    }
    if (status == AUTH_FAILURE) dp_fail(err, "SSH authentication failed");
    return status == AUTH_SUCCESS;
}
