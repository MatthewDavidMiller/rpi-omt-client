/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Session channels (RFC 4254): opening, requests, flow control in both
 * directions, commands with or without a pseudo-terminal, and the `cat`
 * upload fallback. One channel is open at a time; packets for a channel that
 * is already gone are dropped.
 */
#include <string.h>

#include "common/json.h"
#include "deploy/ssh/ssh_internal.h"

/* ----------------------------------------------------------------- source */

long ssh_source_read(ssh_source *s, uint8_t *buf, size_t len, dp_err *err) {
    if (s->file) return dp_file_read(s->file, buf, len, err);
    size_t n = omt_min_size(len, s->len - s->pos);
    memcpy(buf, s->mem + s->pos, n);
    s->pos += n;
    return (long)n;
}

bool ssh_source_rewind(ssh_source *s, dp_err *err) {
    s->pos = 0;
    return s->file ? dp_file_rewind(s->file, err) : true;
}

/* ---------------------------------------------------------------- channel */

static bool send_window_adjust(ssh_channel *ch, dp_err *err) {
    if (ch->consumed < SSH_LOCAL_WINDOW / 2 || ch->close_sent) return true;
    omt_buf msg;
    omt_buf_init(&msg, 16);
    sb_u8(&msg, SSH_MSG_CHANNEL_WINDOW_ADJUST);
    sb_u32(&msg, ch->remote_id);
    sb_u32(&msg, ch->consumed);
    bool ok = ssh_send(ch->t, &msg, err);
    omt_buf_free(&msg);
    ch->consumed = 0;
    return ok;
}

static bool sink(ssh_channel *ch, omt_buf *into, omt_span data, dp_err *err) {
    if (data.len > SSH_LOCAL_WINDOW - ch->consumed) {
        dp_fail(err, "the SSH server overran the channel window");
        return false;
    }
    ch->consumed += (uint32_t)data.len;
    if (into) {
        if (into->len + data.len > ch->out_limit) {
            ch->overflow = true;
            dp_fail(err, "remote command output exceeded 4 MiB");
            return false;
        }
        omt_buf_append(into, data.p, data.len);
        if (into->failed) {
            dp_fail(err, "out of memory");
            return false;
        }
    }
    return send_window_adjust(ch, err);
}

static bool reply_failure(ssh_channel *ch, dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 16);
    sb_u8(&msg, SSH_MSG_CHANNEL_FAILURE);
    sb_u32(&msg, ch->remote_id);
    bool ok = ssh_send(ch->t, &msg, err);
    omt_buf_free(&msg);
    return ok;
}

static bool send_close(ssh_channel *ch, dp_err *err) {
    if (ch->close_sent || !ch->opened) return true;
    omt_buf msg;
    omt_buf_init(&msg, 16);
    sb_u8(&msg, SSH_MSG_CHANNEL_CLOSE);
    sb_u32(&msg, ch->remote_id);
    bool ok = ssh_send(ch->t, &msg, err);
    omt_buf_free(&msg);
    ch->close_sent = true;
    return ok;
}

bool ssh_channel_dispatch(ssh_channel *ch, dp_err *err) {
    ssh_transport *t = ch->t;
    uint8_t type = t->payload.data[0];
    omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1);
    uint32_t recipient;
    if (type < SSH_MSG_CHANNEL_OPEN || type > SSH_MSG_CHANNEL_FAILURE ||
        type == SSH_MSG_CHANNEL_OPEN) {
        if (type == SSH_MSG_CHANNEL_OPEN) {
            /* The server may not open channels to a client that offered no
             * forwarding; refuse whatever it asks for. */
            omt_span kind;
            uint32_t sender;
            if (sr_string(&r, &kind) && sr_u32(&r, &sender)) {
                omt_buf msg;
                omt_buf_init(&msg, 64);
                sb_u8(&msg, SSH_MSG_CHANNEL_OPEN_FAILURE);
                sb_u32(&msg, sender);
                sb_u32(&msg, 1);
                sb_cstring(&msg, "");
                sb_cstring(&msg, "");
                bool ok = ssh_send(t, &msg, err);
                omt_buf_free(&msg);
                return ok;
            }
        }
        dp_fail(err, "unexpected SSH message %u", (unsigned)type);
        return false;
    }
    if (!sr_u32(&r, &recipient)) goto malformed;
    if (recipient != ch->local_id) return true; /* a channel already closed */
    switch (type) {
    case SSH_MSG_CHANNEL_OPEN_CONFIRMATION: {
        uint32_t window, max_packet;
        if (!sr_u32(&r, &ch->remote_id) || !sr_u32(&r, &window) || !sr_u32(&r, &max_packet)) {
            goto malformed;
        }
        ch->remote_window = window;
        ch->remote_max_packet = max_packet;
        ch->opened = true;
        return true;
    }
    case SSH_MSG_CHANNEL_OPEN_FAILURE: ch->open_failed = true; return true;
    case SSH_MSG_CHANNEL_WINDOW_ADJUST: {
        uint32_t add;
        if (!sr_u32(&r, &add)) goto malformed;
        ch->remote_window += add;
        if (ch->remote_window > UINT32_MAX) ch->remote_window = UINT32_MAX;
        return true;
    }
    case SSH_MSG_CHANNEL_DATA: {
        omt_span data;
        if (!sr_string(&r, &data)) goto malformed;
        return sink(ch, ch->out, data, err);
    }
    case SSH_MSG_CHANNEL_EXTENDED_DATA: {
        uint32_t code;
        omt_span data;
        if (!sr_u32(&r, &code) || !sr_string(&r, &data)) goto malformed;
        return sink(ch, code == 1 ? ch->err : NULL, data, err);
    }
    case SSH_MSG_CHANNEL_EOF: ch->eof_received = true; return true;
    case SSH_MSG_CHANNEL_CLOSE: ch->close_received = true; return send_close(ch, err);
    case SSH_MSG_CHANNEL_REQUEST: {
        omt_span name;
        bool want_reply;
        if (!sr_string(&r, &name) || !sr_bool(&r, &want_reply)) goto malformed;
        if (span_is(name, "exit-status")) {
            uint32_t status;
            if (!sr_u32(&r, &status)) goto malformed;
            ch->has_exit = true;
            ch->exit_code = status > INT32_MAX ? 1 : (int)status;
        }
        return !want_reply || reply_failure(ch, err);
    }
    case SSH_MSG_CHANNEL_SUCCESS: ch->reply = 1; return true;
    case SSH_MSG_CHANNEL_FAILURE: ch->reply = -1; return true;
    default: break;
    }
malformed:
    dp_fail(err, "the SSH server sent a malformed channel message");
    return false;
}

int ssh_channel_poll(ssh_channel *ch, uint32_t wait_ms, dp_err *err) {
    ssh_recv_status st = ssh_recv(ch->t, wait_ms, err);
    if (st == SSH_RECV_TIMEOUT) return 0;
    if (st == SSH_RECV_ERROR) return -1;
    return ssh_channel_dispatch(ch, err) ? 1 : -1;
}

/* Waits, servicing the channel, until done() holds or time runs out. */
static bool wait_until(ssh_channel *ch, bool (*done)(const ssh_channel *), uint32_t timeout_ms,
                       const char *timeout_text, dp_err *err) {
    uint64_t deadline = dp_now_ms() + timeout_ms;
    while (!done(ch)) {
        uint64_t now = dp_now_ms();
        if (now >= deadline) {
            dp_fail(err, "%s", timeout_text);
            return false;
        }
        uint64_t left = deadline - now;
        if (ssh_channel_poll(ch, left > 1000 ? 1000u : (uint32_t)left, err) < 0) return false;
    }
    return true;
}

static bool open_settled(const ssh_channel *ch) { return ch->opened || ch->open_failed; }
static bool reply_settled(const ssh_channel *ch) { return ch->reply != 0 || ch->close_received; }

bool ssh_channel_open(ssh_channel *ch, ssh_session *s, omt_buf *out, omt_buf *err_sink,
                      size_t limit, dp_err *err) {
    memset(ch, 0, sizeof(*ch));
    ch->t = &s->t;
    ch->local_id = s->next_channel++;
    ch->out = out;
    ch->err = err_sink;
    ch->out_limit = limit;
    ch->exit_code = 1;
    omt_buf msg;
    omt_buf_init(&msg, 64);
    sb_u8(&msg, SSH_MSG_CHANNEL_OPEN);
    sb_cstring(&msg, "session");
    sb_u32(&msg, ch->local_id);
    sb_u32(&msg, SSH_LOCAL_WINDOW);
    sb_u32(&msg, SSH_LOCAL_MAX_PACKET);
    bool ok = ssh_send(ch->t, &msg, err);
    omt_buf_free(&msg);
    if (!ok || !wait_until(ch, open_settled, (uint32_t)ch->t->idle_ms,
                           "the SSH server did not open a channel", err)) {
        return false;
    }
    if (ch->open_failed) {
        dp_fail(err, "the SSH server refused to open a session channel");
        return false;
    }
    return true;
}

bool ssh_channel_request(ssh_channel *ch, const char *type, bool want_reply, const omt_buf *extra,
                         uint32_t timeout_ms, bool *accepted, dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 64u * 1024u + 1024u);
    sb_u8(&msg, SSH_MSG_CHANNEL_REQUEST);
    sb_u32(&msg, ch->remote_id);
    sb_cstring(&msg, type);
    sb_bool(&msg, want_reply);
    if (extra) omt_buf_append(&msg, extra->data, extra->len);
    ch->reply = 0;
    bool ok = ssh_send(ch->t, &msg, err);
    omt_buf_free(&msg);
    if (!ok) return false;
    if (!want_reply) {
        if (accepted) *accepted = true;
        return true;
    }
    char text[128];
    omt_snprintf(text, sizeof(text), "the SSH server did not answer the %s request", type);
    if (!wait_until(ch, reply_settled, timeout_ms, text, err)) return false;
    if (accepted) *accepted = ch->reply == 1;
    return true;
}

bool ssh_channel_write(ssh_channel *ch, const uint8_t *data, size_t len, uint64_t deadline_ms,
                       const dp_cancel *cancel, dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, SSH_LOCAL_MAX_PACKET + 64);
    bool ok = true;
    while (ok && len > 0) {
        if (dp_cancelled(cancel)) {
            dp_fail_cancelled(err);
            ok = false;
            break;
        }
        if (ch->close_received || ch->eof_sent) {
            dp_fail(err, "the remote side closed the channel");
            ok = false;
            break;
        }
        size_t room = (size_t)omt_min_size((size_t)ch->remote_window, ch->remote_max_packet);
        room = omt_min_size(room, SSH_LOCAL_MAX_PACKET);
        if (room == 0) {
            uint64_t now = dp_now_ms();
            if (now >= deadline_ms) {
                dp_fail(err, "upload timed out waiting for the SSH window");
                ok = false;
                break;
            }
            ok = ssh_channel_poll(ch, 100, err) >= 0;
            continue;
        }
        size_t n = omt_min_size(room, len);
        omt_buf_clear(&msg);
        sb_u8(&msg, SSH_MSG_CHANNEL_DATA);
        sb_u32(&msg, ch->remote_id);
        sb_string(&msg, data, n);
        ok = ssh_send(ch->t, &msg, err);
        ch->remote_window -= n;
        data += n;
        len -= n;
    }
    omt_buf_free_secret(&msg);
    return ok;
}

bool ssh_channel_eof(ssh_channel *ch, dp_err *err) {
    if (ch->eof_sent || ch->close_sent) return true;
    omt_buf msg;
    omt_buf_init(&msg, 16);
    sb_u8(&msg, SSH_MSG_CHANNEL_EOF);
    sb_u32(&msg, ch->remote_id);
    bool ok = ssh_send(ch->t, &msg, err);
    omt_buf_free(&msg);
    ch->eof_sent = true;
    return ok;
}

void ssh_channel_close(ssh_channel *ch) {
    dp_err ignored;
    dp_err_init(&ignored);
    (void)send_close(ch, &ignored);
    dp_err_free(&ignored);
}

/* --------------------------------------------------------------- commands */

void ssh_result_init(ssh_result *r) {
    r->exit_code = 1;
    omt_buf_init(&r->out, DP_OUTPUT_LIMIT * 2 + 16);
    omt_buf_init(&r->err, DP_OUTPUT_LIMIT * 2 + 16);
}

void ssh_result_free(ssh_result *r) {
    omt_buf_free(&r->out);
    omt_buf_free(&r->err);
}

void ssh_result_combined(const ssh_result *r, omt_buf *out) {
    omt_buf_append(out, r->out.data, r->out.len);
    if (r->err.len) {
        if (out->len && out->data[out->len - 1] != '\n') omt_buf_putc(out, '\n');
        omt_buf_append(out, r->err.data, r->err.len);
    }
}

bool ssh_contains_bytes(const uint8_t *haystack, size_t len, const char *needle) {
    size_t n = strlen(needle);
    if (n == 0 || n > len) return false;
    for (size_t i = 0; i + n <= len; i++) {
        if (memcmp(haystack + i, needle, n) == 0) return true;
    }
    return false;
}

static bool run_inner(ssh_session *s, const char *command, const char *marker, const char *input,
                      size_t input_len, bool pty, const dp_cancel *cancel, ssh_result *result,
                      dp_err *err) {
    if (dp_cancelled(cancel)) {
        dp_fail_cancelled(err);
        return false;
    }
    omt_buf out, errb;
    omt_buf_init(&out, DP_OUTPUT_LIMIT + 1);
    omt_buf_init(&errb, DP_OUTPUT_LIMIT + 1);
    ssh_channel ch;
    bool ok = ssh_channel_open(&ch, s, &out, &errb, DP_OUTPUT_LIMIT, err);
    bool accepted = false;
    if (ok && pty) {
        omt_buf req;
        omt_buf_init(&req, 128);
        sb_cstring(&req, "dumb");
        sb_u32(&req, 80);
        sb_u32(&req, 24);
        sb_u32(&req, 0);
        sb_u32(&req, 0);
        sb_string(&req, "\0", 1); /* no modes: TTY_OP_END only */
        ok =
            ssh_channel_request(&ch, "pty-req", true, &req, (uint32_t)s->t.idle_ms, &accepted, err);
        omt_buf_free(&req);
        if (ok && !accepted) {
            dp_fail(err, "the SSH server refused a pseudo-terminal");
            ok = false;
        }
    }
    if (ok) {
        omt_buf req;
        omt_buf_init(&req, 256u * 1024u);
        sb_cstring(&req, command);
        ok = ssh_channel_request(&ch, "exec", true, &req, (uint32_t)s->t.idle_ms, &accepted, err);
        omt_buf_free(&req);
        if (ok && !accepted) {
            dp_fail(err, "the SSH server refused to run the command");
            ok = false;
        }
    }
    uint64_t deadline = dp_now_ms() + SSH_COMMAND_TIMEOUT_MS;
    bool input_sent = input_len == 0;
    if (ok && (input_sent || !marker)) {
        if (input_len)
            ok = ssh_channel_write(&ch, (const uint8_t *)input, input_len, deadline, cancel, err);
        input_sent = true;
        ok = ok && ssh_channel_eof(&ch, err);
    }
    uint64_t last_activity = dp_now_ms();
    while (ok && !ch.close_received) {
        if (dp_cancelled(cancel)) {
            ssh_channel_close(&ch);
            dp_fail_cancelled(err);
            ok = false;
            break;
        }
        int got = ssh_channel_poll(&ch, 100, err);
        if (got < 0) {
            ok = false;
            break;
        }
        uint64_t now = dp_now_ms();
        if (got > 0) last_activity = now;
        if (!input_sent && marker &&
            (ssh_contains_bytes(out.data, out.len, marker) ||
             ssh_contains_bytes(errb.data, errb.len, marker))) {
            ok = ssh_channel_write(&ch, (const uint8_t *)input, input_len, deadline, cancel, err) &&
                 ssh_channel_eof(&ch, err);
            input_sent = true;
        }
        if (now - last_activity > SSH_IDLE_TIMEOUT_MS || now > deadline) {
            ssh_channel_close(&ch);
            dp_fail(err, "remote command produced no output for 60 seconds");
            ok = false;
        }
    }
    if (ok) {
        result->exit_code = ch.has_exit ? ch.exit_code : 1;
        omt_utf8_lossy(&result->out, (const char *)out.data, out.len);
        omt_utf8_lossy(&result->err, (const char *)errb.data, errb.len);
        if (result->out.failed || result->err.failed) {
            dp_fail(err, "out of memory");
            ok = false;
        }
    }
    omt_buf_free_secret(&out);
    omt_buf_free_secret(&errb);
    return ok;
}

bool ssh_run(ssh_session *s, const char *command, const char *input, size_t input_len,
             const dp_cancel *cancel, ssh_result *result, dp_err *err) {
    return run_inner(s, command, NULL, input, input_len, false, cancel, result, err);
}

bool ssh_run_pty_after_marker(ssh_session *s, const char *command, const char *marker,
                              const char *input, size_t input_len, const dp_cancel *cancel,
                              ssh_result *result, dp_err *err) {
    return run_inner(s, command, marker, input, input_len, true, cancel, result, err);
}

/* ------------------------------------------------------- shell fallback */

void ssh_shell_upload_command(omt_buf *out, const char *remote) {
    omt_buf_puts(out, "umask 077 && cat > ");
    dp_shell_quote(out, remote);
}

static bool closed(const ssh_channel *ch) { return ch->close_received; }

bool ssh_shell_upload(ssh_session *s, ssh_source *src, const char *remote, const dp_cancel *cancel,
                      dp_err *err) {
    if (dp_cancelled(cancel)) {
        dp_fail_cancelled(err);
        return false;
    }
    /* cat's output is not read: the exit status is the verdict. */
    ssh_channel ch;
    bool ok = ssh_channel_open(&ch, s, NULL, NULL, 0, err);
    if (ok) {
        omt_buf req, command;
        omt_buf_init(&req, 8192);
        omt_buf_init(&command, 8192);
        ssh_shell_upload_command(&command, remote);
        sb_string(&req, command.data, command.len);
        bool accepted = false;
        ok = ssh_channel_request(&ch, "exec", true, &req, (uint32_t)s->t.idle_ms, &accepted, err);
        if (ok && !accepted) {
            dp_fail(err, "the SSH server refused to run cat");
            ok = false;
        }
        omt_buf_free(&req);
        omt_buf_free(&command);
    }
    uint64_t deadline = dp_now_ms() + SSH_UPLOAD_TIMEOUT_MS;
    uint8_t *chunk = ok ? OPENSSL_malloc(SSH_LOCAL_MAX_PACKET) : NULL;
    if (ok && !chunk) {
        dp_fail(err, "out of memory");
        ok = false;
    }
    while (ok) {
        if (dp_cancelled(cancel)) {
            ssh_channel_close(&ch);
            dp_fail_cancelled(err);
            ok = false;
            break;
        }
        if (dp_now_ms() > deadline) {
            ssh_channel_close(&ch);
            dp_fail(err, "shell upload timed out");
            ok = false;
            break;
        }
        long n = ssh_source_read(src, chunk, SSH_LOCAL_MAX_PACKET, err);
        if (n < 0) {
            ok = false;
            break;
        }
        if (n == 0) break;
        ok = ssh_channel_write(&ch, chunk, (size_t)n, deadline, cancel, err);
    }
    OPENSSL_free(chunk);
    if (ok) ok = ssh_channel_eof(&ch, err);
    if (ok) {
        ok = wait_until(&ch, closed, SSH_UPLOAD_TIMEOUT_MS,
                        "shell upload timed out waiting for exit status", err);
        if (!ok) ssh_channel_close(&ch);
    }
    if (ok && (!ch.has_exit || ch.exit_code != 0)) {
        dp_fail(err, "remote cat exited %d while writing %s", ch.has_exit ? ch.exit_code : 1,
                remote);
        ok = false;
    }
    return ok;
}
