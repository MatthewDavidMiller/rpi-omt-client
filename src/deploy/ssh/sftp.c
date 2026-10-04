/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The SFTP version 3 subset an upload needs (draft-ietf-secsh-filexfer-02):
 * open for writing with create and truncate, pipelined writes, close, and
 * remove when an upload is cancelled.
 */
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

enum {
    SFTP_INIT = 1,
    SFTP_VERSION = 2,
    SFTP_OPEN = 3,
    SFTP_CLOSE = 4,
    SFTP_WRITE = 6,
    SFTP_REMOVE = 13,
    SFTP_STATUS = 101,
    SFTP_HANDLE = 102,
};

#define SFTP_FLAG_WRITE 0x02u
#define SFTP_FLAG_CREAT 0x08u
#define SFTP_FLAG_TRUNC 0x10u
#define SFTP_MAX_PACKET (256u * 1024u)
#define SFTP_CHUNK (32u * 1024u)
#define SFTP_IN_FLIGHT 32u
#define SFTP_SUBSYSTEM_TIMEOUT_MS 15000u

typedef struct {
    ssh_session *s;
    ssh_channel ch;
    omt_buf rx;     /* channel data not yet parsed */
    omt_buf packet; /* the current reply */
    uint32_t next_id;
} sftp;

static bool sftp_send(sftp *f, const omt_buf *body, uint64_t deadline, const dp_cancel *cancel,
                      dp_err *err) {
    uint8_t len[4];
    omt_put_be32(len, (uint32_t)body->len);
    return ssh_channel_write(&f->ch, len, 4, deadline, cancel, err) &&
           ssh_channel_write(&f->ch, body->data, body->len, deadline, cancel, err);
}

/* Reads one reply into f->packet, waiting no more than the idle timeout
 * between packets and the deadline overall. */
static bool sftp_reply(sftp *f, uint64_t deadline, const dp_cancel *cancel, dp_err *err) {
    uint64_t last = dp_now_ms();
    for (;;) {
        if (f->rx.len >= 4) {
            uint32_t len = omt_be32(f->rx.data);
            if (len == 0 || len > SFTP_MAX_PACKET) {
                dp_fail(err, "the SFTP server sent a malformed reply");
                return false;
            }
            if (f->rx.len >= 4u + len) {
                omt_buf_clear(&f->packet);
                omt_buf_append(&f->packet, f->rx.data + 4, len);
                omt_buf_consume(&f->rx, 4u + len);
                return !f->packet.failed;
            }
        }
        if (f->ch.close_received || f->ch.eof_received) {
            dp_fail(err, "the SFTP server closed the session");
            return false;
        }
        if (dp_cancelled(cancel)) {
            dp_fail_cancelled(err);
            return false;
        }
        uint64_t now = dp_now_ms();
        if (now > deadline || now - last > SSH_IDLE_TIMEOUT_MS) {
            dp_fail(err, "SFTP upload timed out");
            return false;
        }
        int got = ssh_channel_poll(&f->ch, 100, err);
        if (got < 0) return false;
        if (got > 0) last = now;
    }
}

/* Checks f->packet is an OK status for request `id`, or for any request
 * when `any_id` is given, which then receives the id answered. */
static bool status_ok(sftp *f, uint32_t id, uint32_t *any_id, const char *what, dp_err *err) {
    omt_span r = omt_span_of(f->packet.data, f->packet.len), message;
    uint8_t type;
    uint32_t got_id, code;
    if (!sr_u8(&r, &type) || type != SFTP_STATUS || !sr_u32(&r, &got_id) ||
        (!any_id && got_id != id) || !sr_u32(&r, &code) || !sr_string(&r, &message)) {
        dp_fail(err, "the SFTP server sent an unexpected reply to %s", what);
        return false;
    }
    if (code != 0) {
        dp_fail(err, "SFTP %s failed: %.*s (status %u)", what, (int)omt_min_size(message.len, 200),
                (const char *)message.p, code);
        return false;
    }
    if (any_id) *any_id = got_id;
    return true;
}

static bool expect_status_ok(sftp *f, uint32_t id, const char *what, dp_err *err) {
    return status_ok(f, id, NULL, what, err);
}

static void header(omt_buf *b, uint8_t type, uint32_t id) {
    omt_buf_clear(b);
    sb_u8(b, type);
    sb_u32(b, id);
}

/* Best effort: close the handle and remove the partial file. */
static void abandon(sftp *f, const omt_span *handle, const char *remote) {
    dp_err ignored;
    dp_err_init(&ignored);
    omt_buf b;
    omt_buf_init(&b, 8192);
    uint64_t deadline = dp_now_ms() + 5000;
    if (handle) {
        header(&b, SFTP_CLOSE, f->next_id++);
        sb_string(&b, handle->p, handle->len);
        (void)sftp_send(f, &b, deadline, NULL, &ignored);
    }
    header(&b, SFTP_REMOVE, f->next_id++);
    sb_cstring(&b, remote);
    if (sftp_send(f, &b, deadline, NULL, &ignored)) {
        /* Wait briefly so the removal lands before the channel goes. */
        for (int i = 0; i < 2; i++) {
            if (!sftp_reply(f, deadline, NULL, &ignored)) break;
        }
    }
    omt_buf_free(&b);
    dp_err_free(&ignored);
}

bool ssh_sftp_upload(ssh_session *s, ssh_source *src, const char *remote, const dp_cancel *cancel,
                     dp_err *err) {
    if (dp_cancelled(cancel)) {
        dp_fail_cancelled(err);
        return false;
    }
    sftp f;
    memset(&f, 0, sizeof(f));
    f.s = s;
    omt_buf_init(&f.rx, SFTP_MAX_PACKET * 2 + SSH_LOCAL_WINDOW);
    omt_buf_init(&f.packet, SFTP_MAX_PACKET);
    omt_buf body, handle_copy;
    omt_buf_init(&body, SFTP_CHUNK + 1024);
    omt_buf_init(&handle_copy, 512);
    uint8_t *chunk = NULL;
    bool ok = ssh_channel_open(&f.ch, s, &f.rx, NULL, SFTP_MAX_PACKET * 2 + SSH_LOCAL_WINDOW, err);
    uint64_t deadline = dp_now_ms() + SSH_UPLOAD_TIMEOUT_MS;
    if (ok) {
        omt_buf req;
        omt_buf_init(&req, 64);
        sb_cstring(&req, "sftp");
        bool accepted = false;
        ok = ssh_channel_request(&f.ch, "subsystem", true, &req, SFTP_SUBSYSTEM_TIMEOUT_MS,
                                 &accepted, err);
        omt_buf_free(&req);
        if (!ok && err && omt_has_prefix(dp_err_text(err), "the SSH server did not answer")) {
            dp_fail(err, "SFTP subsystem request timed out");
        }
        if (ok && !accepted) {
            dp_fail(err, "the SSH server offers no SFTP subsystem");
            ok = false;
        }
    }
    if (ok) {
        omt_buf_clear(&body);
        sb_u8(&body, SFTP_INIT);
        sb_u32(&body, 3);
        ok = sftp_send(&f, &body, deadline, cancel, err) && sftp_reply(&f, deadline, cancel, err);
        if (ok && (f.packet.len < 5 || f.packet.data[0] != SFTP_VERSION ||
                   omt_be32(f.packet.data + 1) < 3)) {
            dp_fail(err, "the SFTP server does not speak version 3");
            ok = false;
        }
    }
    uint32_t open_id = f.next_id++;
    if (ok) {
        header(&body, SFTP_OPEN, open_id);
        sb_cstring(&body, remote);
        sb_u32(&body, SFTP_FLAG_WRITE | SFTP_FLAG_CREAT | SFTP_FLAG_TRUNC);
        sb_u32(&body, 0); /* no attributes */
        ok = sftp_send(&f, &body, deadline, cancel, err) && sftp_reply(&f, deadline, cancel, err);
    }
    omt_span handle = {NULL, 0};
    if (ok) {
        omt_span r = omt_span_of(f.packet.data, f.packet.len);
        uint8_t type;
        uint32_t id;
        if (sr_u8(&r, &type) && type == SFTP_HANDLE && sr_u32(&r, &id) && id == open_id &&
            sr_string(&r, &handle) && handle.len > 0 && handle.len <= 256) {
            omt_buf_append(&handle_copy, handle.p, handle.len);
            handle = omt_span_of(handle_copy.data, handle_copy.len);
        } else {
            if (expect_status_ok(&f, open_id, "open", err)) {
                dp_fail(err, "the SFTP server returned no file handle");
            }
            ok = false;
        }
    }
    /* Outstanding writes, by id modulo the window. Replies to a single
     * handle normally arrive in order, but any outstanding id is accepted. */
    bool pending[SFTP_IN_FLIGHT] = {false};
    uint32_t in_flight = 0;
    uint64_t offset = 0;
    if (ok) {
        chunk = OPENSSL_malloc(SFTP_CHUNK);
        if (!chunk) {
            dp_fail(err, "out of memory");
            ok = false;
        }
    }
    bool at_end = false;
    while (ok && (!at_end || in_flight > 0)) {
        if (dp_cancelled(cancel)) {
            abandon(&f, &handle, remote);
            dp_fail_cancelled(err);
            ok = false;
            break;
        }
        if (!at_end && in_flight < SFTP_IN_FLIGHT) {
            long n = ssh_source_read(src, chunk, SFTP_CHUNK, err);
            if (n < 0) {
                ok = false;
                break;
            }
            if (n == 0) {
                at_end = true;
                continue;
            }
            header(&body, SFTP_WRITE, f.next_id++);
            sb_string(&body, handle.p, handle.len);
            sb_u64(&body, offset);
            sb_string(&body, chunk, (size_t)n);
            offset += (uint64_t)n;
            pending[(f.next_id - 1) % SFTP_IN_FLIGHT] = true;
            ok = sftp_send(&f, &body, deadline, cancel, err);
            in_flight++;
            continue;
        }
        uint32_t id = 0;
        ok = sftp_reply(&f, deadline, cancel, err) && status_ok(&f, 0, &id, "write", err);
        if (ok &&
            (f.next_id - id > SFTP_IN_FLIGHT || f.next_id == id || !pending[id % SFTP_IN_FLIGHT])) {
            dp_fail(err, "the SFTP server answered a request that was not made");
            ok = false;
        }
        if (ok) pending[id % SFTP_IN_FLIGHT] = false;
        in_flight--;
        if (!ok && dp_cancelled(cancel)) abandon(&f, &handle, remote);
    }
    if (ok) {
        uint32_t close_id = f.next_id++;
        header(&body, SFTP_CLOSE, close_id);
        sb_string(&body, handle.p, handle.len);
        ok = sftp_send(&f, &body, deadline, cancel, err) && sftp_reply(&f, deadline, cancel, err) &&
             expect_status_ok(&f, close_id, "close", err);
    }
    OPENSSL_free(chunk);
    if (f.ch.opened) {
        dp_err ignored;
        dp_err_init(&ignored);
        if (ssh_channel_eof(&f.ch, &ignored)) ssh_channel_close(&f.ch);
        /* Let the server's CLOSE arrive so the next channel starts clean. */
        uint64_t until = dp_now_ms() + 5000;
        while (!f.ch.close_received && dp_now_ms() < until) {
            if (ssh_channel_poll(&f.ch, 100, &ignored) < 0) break;
        }
        dp_err_free(&ignored);
    }
    omt_buf_free(&f.rx);
    omt_buf_free(&f.packet);
    omt_buf_free_secret(&body);
    omt_buf_free(&handle_copy);
    return ok;
}
