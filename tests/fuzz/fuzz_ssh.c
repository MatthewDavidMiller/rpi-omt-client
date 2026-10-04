/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Everything the SSH client parses from a server or a file: known_hosts,
 * private keys, public key and signature blobs, EXT_INFO, and the channel
 * messages a session dispatches. The first byte picks the parser.
 */
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#include "deploy/ssh/ssh_internal.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

static void known_hosts(const uint8_t *data, size_t size) {
    static const uint8_t blob[] = {0,   0,   0,  11, 's', 's', 'h', '-', 'e', 'd', '2', '5', '5',
                                   '1', '9', 0,  0,  0,   32,  1,   2,   3,   4,   5,   6,   7,
                                   8,   9,   10, 11, 12,  13,  14,  15,  16,  17,  18,  19,  20,
                                   21,  22,  23, 24, 25,  26,  27,  28,  29,  30,  31,  32};
    ssh_host_status st;
    omt_buf types;
    omt_buf_init(&types, 1u << 20);
    (void)ssh_known_hosts_parse((const char *)data, size, "pi.local", 2222,
                                omt_span_of(blob, sizeof(blob)), &st, &types);
    omt_buf_free(&types);
}

static void private_key(const uint8_t *data, size_t size) {
    ssh_privkey key;
    dp_err err;
    dp_err_init(&err);
    if (ssh_privkey_parse(data, size, "", 0, &key, &err)) ssh_privkey_free(&key);
    dp_err_free(&err);
}

static void public_key(const uint8_t *data, size_t size) {
    omt_span r = omt_span_of(data, size), blob, sig;
    if (!sr_string(&r, &blob)) return;
    sig = r;
    ssh_pubkey key;
    dp_err err;
    dp_err_init(&err);
    if (ssh_pubkey_parse(blob, &key, &err)) {
        static const char *const algs[] = {"ssh-ed25519", "ecdsa-sha2-nistp256", "rsa-sha2-512"};
        for (size_t i = 0; i < 3; i++) {
            (void)ssh_pubkey_verify(&key, algs[i], sig, (const uint8_t *)"h", 1, &err);
        }
        ssh_pubkey_free(&key);
    }
    dp_err_free(&err);
}

/* A transport whose socket is one end of a socketpair, so replies the
 * dispatcher sends land somewhere harmless. */
static void channel_messages(const uint8_t *data, size_t size) {
    int fds[2];
    if (size == 0 || socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0, fds) != 0) return;
    ssh_session s;
    memset(&s, 0, sizeof(s));
    ssh_transport_init(&s.t);
    s.t.sock = fds[0];
    s.t.idle_ms = 10;
    omt_buf out, errb;
    omt_buf_init(&out, DP_OUTPUT_LIMIT + 1);
    omt_buf_init(&errb, DP_OUTPUT_LIMIT + 1);
    ssh_channel ch;
    memset(&ch, 0, sizeof(ch));
    ch.t = &s.t;
    ch.out = &out;
    ch.err = &errb;
    ch.out_limit = DP_OUTPUT_LIMIT;
    ch.opened = true;
    /* Messages are length-prefixed in the input. */
    omt_span r = omt_span_of(data, size), msg;
    dp_err err;
    dp_err_init(&err);
    while (sr_string(&r, &msg) && msg.len > 0) {
        omt_buf_clear(&s.t.payload);
        omt_buf_append(&s.t.payload, msg.p, msg.len);
        if (s.t.payload.data[0] == SSH_MSG_EXT_INFO) {
            (void)ssh_handle_ext_info(&s.t, &err);
        } else {
            (void)ssh_channel_dispatch(&ch, &err);
        }
        char sink[65536];
        while (read(fds[1], sink, sizeof(sink)) > 0) {}
    }
    dp_err_free(&err);
    omt_buf_free(&out);
    omt_buf_free(&errb);
    ssh_transport_free(&s.t); /* closes fds[0] */
    close(fds[1]);
}

static void readers(const uint8_t *data, size_t size) {
    omt_buf b;
    omt_buf_init(&b, size + 16);
    (void)ssh_base64_decode((const char *)data, size, true, &b);
    omt_buf_free(&b);
    omt_span r = omt_span_of(data, size), s;
    BIGNUM *bn = NULL;
    while (r.len) {
        if (sr_mpint_bn(&r, &bn)) {
            BN_free(bn);
            bn = NULL;
        } else if (!sr_string(&r, &s)) {
            break;
        }
    }
}

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    if (size == 0) return 0;
    const uint8_t *rest = data + 1;
    size_t n = size - 1;
    switch (data[0] % 5) {
    case 0: known_hosts(rest, n); break;
    case 1: private_key(rest, n); break;
    case 2: public_key(rest, n); break;
    case 3: channel_messages(rest, n); break;
    default: readers(rest, n); break;
    }
    return 0;
}
