/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The SSH transport (RFC 4253): identification exchange, the binary packet
 * protocol, and the transport messages a session never needs to see.
 */
#include <openssl/rand.h>
#include <stdlib.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

#define IDENT "SSH-2.0-rpi_omt_deploy"
#define MAX_PREFACE_LINES 64u
#define MAX_LINE 255u

void ssh_transport_init(ssh_transport *t) {
    memset(t, 0, sizeof(*t));
    t->sock = DP_SOCK_INVALID;
    omt_buf_init(&t->v_c, 256);
    omt_buf_init(&t->v_s, 256);
    omt_buf_init(&t->i_c, 64u * 1024u);
    omt_buf_init(&t->i_s, 64u * 1024u);
    omt_buf_init(&t->session_id, 64);
    omt_buf_init(&t->host_key, 64u * 1024u);
    omt_buf_init(&t->preferred_hostkeys, 1024);
    omt_buf_init(&t->server_sig_algs, 64u * 1024u);
    omt_buf_init(&t->rbuf, 2u * SSH_MAX_PACKET + 4096u);
    omt_buf_init(&t->payload, SSH_MAX_PACKET);
    omt_buf_init(&t->sbuf, SSH_MAX_PACKET + 1024u);
    t->idle_ms = SSH_IDLE_TIMEOUT_MS;
}

void ssh_transport_free(ssh_transport *t) {
    dp_sock_close(t->sock);
    t->sock = DP_SOCK_INVALID;
    free(t->host);
    free(t->known_hosts);
    t->host = t->known_hosts = NULL;
    omt_buf_free(&t->v_c);
    omt_buf_free(&t->v_s);
    omt_buf_free(&t->i_c);
    omt_buf_free(&t->i_s);
    omt_buf_free_secret(&t->session_id);
    omt_buf_free(&t->host_key);
    omt_buf_free(&t->preferred_hostkeys);
    omt_buf_free(&t->server_sig_algs);
    omt_buf_free_secret(&t->rbuf);
    omt_buf_free_secret(&t->payload);
    omt_buf_free_secret(&t->sbuf);
    ssh_cipher_free(&t->send);
    ssh_cipher_free(&t->recv);
}

/* Reads more bytes into rbuf, waiting until the deadline at most. */
static ssh_recv_status fill(ssh_transport *t, uint64_t deadline_ms, dp_err *err) {
    uint8_t chunk[16384];
    for (;;) {
        if (dp_cancelled(t->cancel)) {
            dp_fail_cancelled(err);
            return SSH_RECV_ERROR;
        }
        uint64_t now = dp_now_ms();
        if (now >= deadline_ms) return SSH_RECV_TIMEOUT;
        uint64_t left = deadline_ms - now;
        long n =
            dp_sock_recv(t->sock, chunk, sizeof(chunk), left > 100 ? 100u : (uint32_t)left, err);
        if (n == -2) continue;
        if (n < 0) return SSH_RECV_ERROR;
        if (n == 0) {
            dp_fail(err, "the SSH server closed the connection");
            return SSH_RECV_ERROR;
        }
        omt_buf_append(&t->rbuf, chunk, (size_t)n);
        if (t->rbuf.failed) {
            dp_fail(err, "SSH receive buffer overflow");
            return SSH_RECV_ERROR;
        }
        return SSH_RECV_OK;
    }
}

static bool send_raw(ssh_transport *t, const void *data, size_t len, dp_err *err) {
    return dp_sock_send(t->sock, data, len, (uint32_t)t->idle_ms, err);
}

/* ---------------------------------------------------------- identification */

static bool exchange_identification(ssh_transport *t, uint64_t deadline, dp_err *err) {
    omt_buf_puts(&t->v_c, IDENT);
    if (!send_raw(t, IDENT "\r\n", sizeof(IDENT "\r\n") - 1, err)) return false;
    for (unsigned lines = 0; lines < MAX_PREFACE_LINES;) {
        uint8_t *nl = t->rbuf.len ? memchr(t->rbuf.data, '\n', t->rbuf.len) : NULL;
        if (!nl) {
            if (t->rbuf.len > MAX_LINE) {
                dp_fail(err, "the SSH server sent an overlong identification line");
                return false;
            }
            ssh_recv_status st = fill(t, deadline, err);
            if (st == SSH_RECV_TIMEOUT) dp_fail(err, "SSH connection timed out");
            if (st != SSH_RECV_OK) return false;
            continue;
        }
        size_t n = (size_t)(nl - t->rbuf.data);
        size_t text = n > 0 && t->rbuf.data[n - 1] == '\r' ? n - 1 : n;
        if (n + 1 > MAX_LINE) {
            dp_fail(err, "the SSH server sent an overlong identification line");
            return false;
        }
        bool is_version = text >= 4 && memcmp(t->rbuf.data, "SSH-", 4) == 0;
        if (is_version) {
            if (!(text >= 8 && memcmp(t->rbuf.data, "SSH-2.0-", 8) == 0) &&
                !(text >= 9 && memcmp(t->rbuf.data, "SSH-1.99-", 9) == 0)) {
                dp_fail(err, "the server does not speak SSH protocol 2");
                return false;
            }
            for (size_t i = 0; i < text; i++) {
                if (t->rbuf.data[i] < 0x20 || t->rbuf.data[i] > 0x7e) {
                    dp_fail(err, "the SSH server sent an invalid identification string");
                    return false;
                }
            }
            omt_buf_append(&t->v_s, t->rbuf.data, text);
            omt_buf_consume(&t->rbuf, n + 1);
            return !t->v_s.failed;
        }
        omt_buf_consume(&t->rbuf, n + 1);
        lines++;
    }
    dp_fail(err, "the SSH server sent no identification string");
    return false;
}

/* ----------------------------------------------------------------- packets */

bool ssh_send(ssh_transport *t, const omt_buf *payload, dp_err *err) {
    if (payload->failed || payload->len == 0 || payload->len > SSH_MAX_PACKET - 64) {
        dp_fail(err, "SSH packet is too large or could not be built");
        return false;
    }
    size_t block = ssh_cipher_block(&t->send);
    size_t aad = ssh_cipher_aead(&t->send) ? 4 : 0;
    size_t unpadded = 4 + 1 + payload->len - aad;
    size_t padding = block - unpadded % block;
    if (padding < 4) padding += block;
    size_t packet_len = 1 + payload->len + padding;
    size_t total = 4 + packet_len;
    size_t tag = ssh_cipher_tag_len(&t->send);
    omt_buf *b = &t->sbuf;
    omt_buf_clear(b);
    if (!omt_buf_reserve(b, total + tag)) {
        dp_fail(err, "out of memory");
        return false;
    }
    sb_u32(b, (uint32_t)packet_len);
    sb_u8(b, (uint8_t)padding);
    omt_buf_append(b, payload->data, payload->len);
    uint8_t pad[64];
    if (RAND_bytes(pad, (int)padding) != 1) {
        dp_fail(err, "random padding is unavailable");
        return false;
    }
    omt_buf_append(b, pad, padding);
    uint8_t mac[64];
    if (!ssh_cipher_seal(&t->send, t->send_seq, b->data, total, mac)) {
        dp_fail(err, "SSH packet encryption failed");
        return false;
    }
    omt_buf_append(b, mac, tag);
    t->send_seq++;
    bool ok = send_raw(t, b->data, b->len, err);
    omt_buf_clear(b);
    return ok;
}

ssh_recv_status ssh_read_packet(ssh_transport *t, uint64_t deadline_ms, dp_err *err) {
    for (;;) {
        if (!t->have_len) {
            size_t first = ssh_cipher_first_len(&t->recv);
            if (t->rbuf.len >= first) {
                uint32_t len;
                if (!ssh_cipher_length(&t->recv, t->recv_seq, t->rbuf.data, &len)) {
                    dp_fail(err, "SSH packet decryption failed");
                    return SSH_RECV_ERROR;
                }
                size_t block = ssh_cipher_block(&t->recv);
                size_t aligned = ssh_cipher_aead(&t->recv) ? len : (size_t)len + 4;
                if (len < 5 || len > SSH_MAX_PACKET || aligned % block != 0) {
                    dp_fail(err, "the SSH server sent a malformed packet");
                    return SSH_RECV_ERROR;
                }
                t->pkt_len = len;
                t->have_len = true;
            }
        }
        if (t->have_len) {
            size_t tag = ssh_cipher_tag_len(&t->recv);
            size_t total = 4 + (size_t)t->pkt_len;
            if (t->rbuf.len >= total + tag) {
                uint8_t *packet = t->rbuf.data;
                if (!ssh_cipher_open(&t->recv, t->recv_seq, packet, total, packet + total)) {
                    dp_fail(err, "SSH packet authentication failed");
                    return SSH_RECV_ERROR;
                }
                uint8_t padding = packet[4];
                if (padding < 4 || (size_t)padding + 1 >= t->pkt_len) {
                    dp_fail(err, "the SSH server sent a malformed packet");
                    return SSH_RECV_ERROR;
                }
                size_t payload_len = t->pkt_len - 1u - padding;
                omt_buf_clear(&t->payload);
                omt_buf_append(&t->payload, packet + 5, payload_len);
                OPENSSL_cleanse(packet, total + tag);
                omt_buf_consume(&t->rbuf, total + tag);
                t->have_len = false;
                t->recv_seq++;
                if (t->payload.failed) {
                    dp_fail(err, "out of memory");
                    return SSH_RECV_ERROR;
                }
                return SSH_RECV_OK;
            }
        }
        ssh_recv_status st = fill(t, deadline_ms, err);
        if (st != SSH_RECV_OK) return st;
    }
}

void ssh_disconnect(ssh_transport *t, uint32_t reason, const char *text) {
    if (t->sock == DP_SOCK_INVALID || !t->recv.alg) return;
    omt_buf msg;
    omt_buf_init(&msg, 1024);
    sb_u8(&msg, SSH_MSG_DISCONNECT);
    sb_u32(&msg, reason);
    sb_cstring(&msg, text);
    sb_cstring(&msg, "");
    dp_err ignored;
    dp_err_init(&ignored);
    t->idle_ms = 2000;
    (void)ssh_send(t, &msg, &ignored);
    dp_err_free(&ignored);
    omt_buf_free(&msg);
}

static void describe_disconnect(ssh_transport *t, dp_err *err) {
    omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1), text;
    uint32_t reason = 0;
    if (!sr_u32(&r, &reason) || !sr_string(&r, &text)) text.len = 0;
    omt_buf safe;
    omt_buf_init(&safe, 1024);
    for (size_t i = 0; i < text.len && i < 512; i++) {
        uint8_t c = text.p[i];
        omt_buf_putc(&safe, c >= 0x20 && c < 0x7f ? c : '?');
    }
    dp_fail(err, "the SSH server disconnected (reason %u): %s", reason, omt_buf_cstr(&safe));
    omt_buf_free(&safe);
}

bool ssh_handle_ext_info(ssh_transport *t, dp_err *err) {
    omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1);
    uint32_t count;
    if (!sr_u32(&r, &count) || count > 1024) goto bad;
    for (uint32_t i = 0; i < count; i++) {
        omt_span name, value;
        if (!sr_string(&r, &name) || !sr_string(&r, &value)) goto bad;
        if (span_is(name, "server-sig-algs")) {
            omt_buf_clear(&t->server_sig_algs);
            omt_buf_append(&t->server_sig_algs, value.p, value.len);
        }
    }
    return true;
bad:
    dp_fail(err, "the SSH server sent a malformed EXT_INFO");
    return false;
}

ssh_recv_status ssh_recv(ssh_transport *t, uint32_t wait_ms, dp_err *err) {
    uint64_t deadline = dp_now_ms() + wait_ms;
    for (;;) {
        ssh_recv_status st = ssh_read_packet(t, deadline, err);
        if (st != SSH_RECV_OK) return st;
        uint8_t type = t->payload.data[0];
        switch (type) {
        case SSH_MSG_DISCONNECT: describe_disconnect(t, err); return SSH_RECV_ERROR;
        case SSH_MSG_IGNORE:
        case SSH_MSG_DEBUG: continue;
        case SSH_MSG_UNIMPLEMENTED:
            dp_fail(err, "the SSH server rejected a message as unimplemented");
            return SSH_RECV_ERROR;
        case SSH_MSG_EXT_INFO:
            if (!ssh_handle_ext_info(t, err)) return SSH_RECV_ERROR;
            continue;
        case SSH_MSG_KEXINIT:
            /* A server-initiated re-key, handled inline before anything else
             * is sent. */
            if (!ssh_kex_run(t, true, err)) return SSH_RECV_ERROR;
            continue;
        case SSH_MSG_GLOBAL_REQUEST: {
            omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1), name;
            bool want_reply = false;
            if (!sr_string(&r, &name) || !sr_bool(&r, &want_reply)) {
                dp_fail(err, "the SSH server sent a malformed global request");
                return SSH_RECV_ERROR;
            }
            if (want_reply) {
                omt_buf reply;
                omt_buf_init(&reply, 16);
                sb_u8(&reply, SSH_MSG_REQUEST_FAILURE);
                bool ok = ssh_send(t, &reply, err);
                omt_buf_free(&reply);
                if (!ok) return SSH_RECV_ERROR;
            }
            continue;
        }
        default: return SSH_RECV_OK;
        }
    }
}

bool ssh_recv_wait(ssh_transport *t, dp_err *err) {
    ssh_recv_status st = ssh_recv(t, (uint32_t)t->idle_ms, err);
    if (st == SSH_RECV_TIMEOUT) dp_fail(err, "the SSH server stopped responding");
    return st == SSH_RECV_OK;
}

/* --------------------------------------------------------------- opening */

/* Host-key algorithms in preference order, with those whose key type
 * known_hosts already lists for this host moved to the front: offering a
 * type the operator never verified would only end in a refusal. */
static void order_hostkeys(ssh_transport *t, const omt_buf *known_types) {
    omt_span known = omt_span_of(known_types->data, known_types->len);
    const char *all = SSH_HOSTKEY_LIST;
    for (int pass = 0; pass < 2; pass++) {
        const char *p = all;
        while (*p) {
            const char *comma = strchr(p, ',');
            size_t n = comma ? (size_t)(comma - p) : strlen(p);
            char alg[64];
            omt_snprintf(alg, sizeof(alg), "%.*s", (int)n, p);
            const char *type =
                (strcmp(alg, "rsa-sha2-512") == 0 || strcmp(alg, "rsa-sha2-256") == 0) ? "ssh-rsa"
                                                                                       : alg;
            bool is_known = namelist_has(known, type);
            if ((pass == 0) == is_known) {
                if (t->preferred_hostkeys.len) omt_buf_putc(&t->preferred_hostkeys, ',');
                omt_buf_puts(&t->preferred_hostkeys, alg);
            }
            p += n + (comma ? 1 : 0);
        }
    }
}

bool ssh_transport_open(ssh_transport *t, const dp_connection *c, const dp_cancel *cancel,
                        dp_err *err) {
    t->cancel = cancel;
    t->host = dp_strdup(c->host);
    t->port = c->port;
    omt_buf path;
    omt_buf_init(&path, 8192);
    bool ok = ssh_known_hosts_path(c, &path, err);
    if (ok) t->known_hosts = dp_strdup(omt_buf_cstr(&path));
    omt_buf_free(&path);
    if (!ok || !t->host || !t->known_hosts) {
        if (ok) dp_fail(err, "out of memory");
        return false;
    }
    omt_buf types;
    omt_buf_init(&types, 4096);
    ok = ssh_known_hosts_types(t->known_hosts, c->host, c->port, &types, err);
    if (ok) order_hostkeys(t, &types);
    omt_buf_free(&types);
    if (!ok) return false;

    uint64_t deadline = dp_now_ms() + SSH_CONNECT_TIMEOUT_MS;
    if (!dp_sock_connect(c->host, c->port, SSH_CONNECT_TIMEOUT_MS, &t->sock, err)) return false;
    /* The handshake shares the connect budget. */
    uint64_t saved_idle = t->idle_ms;
    t->idle_ms = SSH_CONNECT_TIMEOUT_MS;
    ok = exchange_identification(t, deadline, err) && ssh_kex_run(t, false, err);
    t->idle_ms = saved_idle;
    if (ok && dp_now_ms() > deadline) {
        dp_fail(err, "SSH connection timed out");
        ok = false;
    }
    return ok;
}
