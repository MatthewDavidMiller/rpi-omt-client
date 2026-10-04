/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/dbus.h"

#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include "common/json.h"
#include "common/proc.h"

#define DEFAULT_SYSTEM_BUS "unix:path=/var/run/dbus/system_bus_socket"

enum {
    FIELD_PATH = 1,
    FIELD_INTERFACE = 2,
    FIELD_MEMBER = 3,
    FIELD_ERROR_NAME = 4,
    FIELD_REPLY_SERIAL = 5,
    FIELD_DESTINATION = 6,
    FIELD_SENDER = 7,
    FIELD_SIGNATURE = 8,
    FIELD_UNIX_FDS = 9,
};

#define FLAG_NO_REPLY_EXPECTED 0x1

/* ------------------------------------------------------------- reading */

static uint32_t load32(const uint8_t *p, bool big) { return big ? omt_be32(p) : omt_le32(p); }

void omt_dbus_message_free(omt_dbus_message *m) {
    free(m->raw);
    memset(m, 0, sizeof(*m));
}

static bool align_to(size_t *pos, size_t end, size_t alignment, const uint8_t *raw) {
    size_t aligned = (*pos + alignment - 1) & ~(alignment - 1);
    if (aligned > end) return false;
    /* The specification requires padding bytes to be zero. */
    for (size_t i = *pos; i < aligned; i++)
        if (raw[i] != 0) return false;
    *pos = aligned;
    return true;
}

/* Reads a u32-length string (or a u8-length signature) at *pos. */
static const char *read_str(const uint8_t *raw, size_t *pos, size_t end, bool big, bool signature) {
    size_t len;
    if (signature) {
        if (*pos >= end) return NULL;
        len = raw[*pos];
        *pos += 1;
    } else {
        if (!align_to(pos, end, 4, raw) || end - *pos < 4) return NULL;
        len = load32(raw + *pos, big);
        *pos += 4;
    }
    if (len >= end - *pos || raw[*pos + len] != 0) return NULL;
    const char *s = (const char *)raw + *pos;
    if (memchr(s, 0, len) || !omt_utf8_valid(s, len)) return NULL;
    *pos += len + 1;
    return s;
}

size_t omt_dbus_message_length(const uint8_t *data, size_t len) {
    if (len < 16) return 0;
    bool big;
    if (data[0] == 'l')
        big = false;
    else if (data[0] == 'B')
        big = true;
    else
        return 0;
    if (data[3] != 1) return 0;
    uint64_t body = load32(data + 4, big);
    uint64_t fields = load32(data + 12, big);
    uint64_t header = 16 + fields;
    header = (header + 7) & ~(uint64_t)7;
    uint64_t total = header + body;
    if (total > OMT_DBUS_MAX_MESSAGE) return 0;
    return (size_t)total;
}

bool omt_dbus_parse(uint8_t *raw, size_t len, omt_dbus_message *m) {
    memset(m, 0, sizeof(*m));
    size_t total = omt_dbus_message_length(raw, len);
    if (total == 0 || total != len) return false;
    m->raw = raw;
    m->len = len;
    m->big_endian = raw[0] == 'B';
    m->type = raw[1];
    m->serial = load32(raw + 8, m->big_endian);
    if (m->type < 1 || m->type > 4 || m->serial == 0) return false;
    size_t fields_len = load32(raw + 12, m->big_endian);
    size_t pos = 16, end = 16 + fields_len;
    while (pos < end) {
        if (!align_to(&pos, end, 8, raw) || pos >= end) return false;
        uint8_t code = raw[pos++];
        const char *sig = read_str(raw, &pos, end, m->big_endian, true);
        if (!sig || strlen(sig) != 1) return false;
        switch (code) {
        case FIELD_PATH:
        case FIELD_INTERFACE:
        case FIELD_MEMBER:
        case FIELD_ERROR_NAME:
        case FIELD_DESTINATION:
        case FIELD_SENDER: {
            char want = code == FIELD_PATH ? 'o' : 's';
            if (sig[0] != want) return false;
            const char *s = read_str(raw, &pos, end, m->big_endian, false);
            if (!s) return false;
            if (code == FIELD_PATH) m->path = s;
            if (code == FIELD_INTERFACE) m->interface = s;
            if (code == FIELD_MEMBER) m->member = s;
            break;
        }
        case FIELD_SIGNATURE: {
            if (sig[0] != 'g') return false;
            const char *s = read_str(raw, &pos, end, m->big_endian, true);
            if (!s) return false;
            m->signature = s;
            break;
        }
        case FIELD_REPLY_SERIAL:
        case FIELD_UNIX_FDS:
            if (sig[0] != 'u' || !align_to(&pos, end, 4, raw) || end - pos < 4) return false;
            if (code == FIELD_REPLY_SERIAL) m->reply_serial = load32(raw + pos, m->big_endian);
            if (code == FIELD_UNIX_FDS && load32(raw + pos, m->big_endian) != 0) return false;
            pos += 4;
            break;
        default:
            /* Unknown fields must be skipped; only simple types are accepted
             * so the skip stays a fixed-size step. */
            if (sig[0] == 's' || sig[0] == 'o') {
                if (!read_str(raw, &pos, end, m->big_endian, false)) return false;
            } else if (sig[0] == 'g') {
                if (!read_str(raw, &pos, end, m->big_endian, true)) return false;
            } else if (sig[0] == 'u' || sig[0] == 'i') {
                if (!align_to(&pos, end, 4, raw) || end - pos < 4) return false;
                pos += 4;
            } else if (sig[0] == 'y') {
                if (pos >= end) return false;
                pos += 1;
            } else {
                return false;
            }
        }
    }
    size_t body = (end + 7) & ~(size_t)7;
    for (size_t i = end; i < body; i++)
        if (raw[i] != 0) return false;
    m->body_offset = body;
    m->body_len = len - body;
    if (!m->signature) m->signature = "";
    return true;
}

void omt_dbus_reader_init(omt_dbus_reader *r, const omt_dbus_message *m) {
    r->m = m;
    r->pos = m->body_offset;
    r->end = m->body_offset + m->body_len;
    r->failed = false;
}

static bool reader_align(omt_dbus_reader *r, size_t n) {
    if (r->failed) return false;
    /* Body alignment is relative to the body start, which is 8-aligned. */
    if (!align_to(&r->pos, r->end, n, r->m->raw)) r->failed = true;
    return !r->failed;
}

uint32_t omt_dbus_read_u32(omt_dbus_reader *r) {
    if (!reader_align(r, 4) || r->end - r->pos < 4) {
        r->failed = true;
        return 0;
    }
    uint32_t v = load32(r->m->raw + r->pos, r->m->big_endian);
    r->pos += 4;
    return v;
}

int32_t omt_dbus_read_i32(omt_dbus_reader *r) { return (int32_t)omt_dbus_read_u32(r); }

uint16_t omt_dbus_read_u16(omt_dbus_reader *r) {
    if (!reader_align(r, 2) || r->end - r->pos < 2) {
        r->failed = true;
        return 0;
    }
    const uint8_t *p = r->m->raw + r->pos;
    uint16_t v = r->m->big_endian ? (uint16_t)((unsigned)p[0] << 8 | p[1]) : omt_le16(p);
    r->pos += 2;
    return v;
}

bool omt_dbus_read_bool(omt_dbus_reader *r) {
    uint32_t v = omt_dbus_read_u32(r);
    if (v > 1) r->failed = true;
    return v == 1;
}

const char *omt_dbus_read_string(omt_dbus_reader *r) {
    if (r->failed) return NULL;
    const char *s = read_str(r->m->raw, &r->pos, r->end, r->m->big_endian, false);
    if (!s) r->failed = true;
    return s;
}

void omt_dbus_skip_aay(omt_dbus_reader *r) {
    uint32_t outer = omt_dbus_read_u32(r);
    if (r->failed) return;
    /* Elements of an `ay` array are 4-aligned (their u32 length). */
    if (!reader_align(r, 4)) return;
    if (outer > r->end - r->pos) {
        r->failed = true;
        return;
    }
    size_t stop = r->pos + outer;
    while (r->pos < stop && !r->failed) {
        uint32_t inner = omt_dbus_read_u32(r);
        if (r->failed || inner > stop - r->pos) {
            r->failed = true;
            return;
        }
        r->pos += inner;
    }
    if (r->pos != stop) r->failed = true;
}

/* ------------------------------------------------------------- writing */

static void w_align(omt_buf *b, size_t n) {
    while (b->len % n) omt_buf_putc(b, 0);
}
static void w_u32(omt_buf *b, uint32_t v) {
    w_align(b, 4);
    uint8_t p[4];
    omt_put_le32(p, v);
    omt_buf_append(b, p, 4);
}
static void w_str(omt_buf *b, const char *s) {
    w_u32(b, (uint32_t)strlen(s));
    omt_buf_append(b, s, strlen(s) + 1);
}
static void w_sig(omt_buf *b, const char *s) {
    omt_buf_putc(b, (uint8_t)strlen(s));
    omt_buf_append(b, s, strlen(s) + 1);
}
static void w_field_str(omt_buf *b, uint8_t code, char type, const char *s) {
    w_align(b, 8);
    omt_buf_putc(b, code);
    char sig[2] = {type, 0};
    w_sig(b, sig);
    if (type == 'g')
        w_sig(b, s);
    else
        w_str(b, s);
}

bool omt_dbus_build_call(omt_buf *out, uint32_t serial, uint8_t flags, const char *destination,
                         const char *path, const char *interface, const char *member,
                         const omt_dbus_arg *args, size_t argc) {
    char signature[32];
    if (argc >= sizeof(signature)) return false;
    for (size_t i = 0; i < argc; i++) {
        if (args[i].type != 'i' && args[i].type != 'u' && args[i].type != 's' &&
            args[i].type != 'o')
            return false;
        signature[i] = args[i].type;
    }
    signature[argc] = 0;

    omt_buf body;
    omt_buf_init(&body, OMT_DBUS_MAX_MESSAGE);
    for (size_t i = 0; i < argc; i++) {
        if (args[i].type == 'i')
            w_u32(&body, (uint32_t)args[i].v.i);
        else if (args[i].type == 'u')
            w_u32(&body, args[i].v.u);
        else
            w_str(&body, args[i].v.s);
    }

    omt_buf_clear(out);
    uint8_t fixed[12] = {'l', OMT_DBUS_METHOD_CALL, flags, 1};
    omt_put_le32(fixed + 4, (uint32_t)body.len);
    omt_put_le32(fixed + 8, serial);
    omt_buf_append(out, fixed, sizeof(fixed));
    w_u32(out, 0); /* field array length, patched below */
    size_t fields_start = out->len;
    w_field_str(out, FIELD_PATH, 'o', path);
    if (interface) w_field_str(out, FIELD_INTERFACE, 's', interface);
    w_field_str(out, FIELD_MEMBER, 's', member);
    if (destination) w_field_str(out, FIELD_DESTINATION, 's', destination);
    if (argc) w_field_str(out, FIELD_SIGNATURE, 'g', signature);
    size_t fields_len = out->len - fields_start;
    w_align(out, 8);
    omt_buf_append(out, body.data ? body.data : (const uint8_t *)"", body.len);
    bool ok = !out->failed && !body.failed;
    if (ok) omt_put_le32(out->data + 12, (uint32_t)fields_len);
    omt_buf_free(&body);
    return ok;
}

/* ------------------------------------------------------------ transport */

static int wait_fd(int fd, short events, uint64_t deadline_ms) {
    for (;;) {
        uint64_t left = omt_remaining_ms(deadline_ms);
        if (left == 0) return 0;
        struct pollfd p = {fd, events, 0};
        int rc = poll(&p, 1, (int)omt_min_size(left, INT32_MAX));
        if (rc < 0 && errno == EINTR) continue;
        return rc;
    }
}

static bool send_all(int fd, const uint8_t *data, size_t len, uint64_t deadline_ms, omt_err *err) {
    while (len) {
        ssize_t n = send(fd, data, len, MSG_NOSIGNAL | MSG_DONTWAIT);
        if (n > 0) {
            data += n;
            len -= (size_t)n;
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            if (wait_fd(fd, POLLOUT, deadline_ms) <= 0) {
                omt_err_set(err, "D-Bus write timed out");
                return false;
            }
            continue;
        }
        omt_err_os(err, n < 0 ? errno : EPIPE);
        return false;
    }
    return true;
}

/* Reads one CRLF-terminated authentication line. */
static bool read_line(int fd, char *out, size_t size, uint64_t deadline_ms, omt_err *err) {
    size_t n = 0;
    while (n + 1 < size) {
        if (wait_fd(fd, POLLIN, deadline_ms) <= 0) {
            omt_err_set(err, "D-Bus authentication timed out");
            return false;
        }
        char c;
        ssize_t r = recv(fd, &c, 1, MSG_DONTWAIT);
        if (r == 0) {
            omt_err_set(err, "D-Bus connection closed during authentication");
            return false;
        }
        if (r < 0) {
            if (errno == EINTR || errno == EAGAIN) continue;
            omt_err_os(err, errno);
            return false;
        }
        if (c == '\n' && n > 0 && out[n - 1] == '\r') {
            out[n - 1] = 0;
            return true;
        }
        out[n++] = c;
    }
    omt_err_set(err, "D-Bus authentication line too long");
    return false;
}

/* Decodes the %xx escapes D-Bus addresses use for values. */
static bool unescape(const char *in, size_t len, char *out, size_t size) {
    size_t n = 0;
    for (size_t i = 0; i < len; i++) {
        if (n + 1 >= size) return false;
        if (in[i] == '%') {
            unsigned v;
            if (i + 2 >= len) return false;
            if (sscanf(in + i + 1, "%2x", &v) != 1) return false;
            out[n++] = (char)v;
            i += 2;
        } else {
            out[n++] = in[i];
        }
    }
    out[n] = 0;
    return true;
}

static int connect_address(const char *address, size_t len, uint64_t deadline_ms) {
    if (len < 5 || memcmp(address, "unix:", 5) != 0) return -1;
    struct sockaddr_un sun;
    memset(&sun, 0, sizeof(sun));
    sun.sun_family = AF_UNIX;
    socklen_t sun_len = 0;
    const char *p = address + 5, *end = address + len;
    while (p < end) {
        const char *comma = memchr(p, ',', (size_t)(end - p));
        const char *stop = comma ? comma : end;
        const char *eq = memchr(p, '=', (size_t)(stop - p));
        if (eq) {
            size_t key = (size_t)(eq - p);
            char value[sizeof(sun.sun_path)];
            if (!unescape(eq + 1, (size_t)(stop - eq - 1), value, sizeof(value))) return -1;
            if (key == 4 && memcmp(p, "path", 4) == 0) {
                omt_strlcpy(sun.sun_path, value, sizeof(sun.sun_path));
                sun_len = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(value) + 1);
            } else if (key == 8 && memcmp(p, "abstract", 8) == 0) {
                size_t vlen = strlen(value);
                if (vlen + 1 > sizeof(sun.sun_path)) return -1;
                sun.sun_path[0] = 0;
                memcpy(sun.sun_path + 1, value, vlen);
                sun_len = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + 1 + vlen);
            }
        }
        p = comma ? comma + 1 : end;
    }
    if (sun_len == 0) return -1;
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) return -1;
    if (connect(fd, (struct sockaddr *)&sun, sun_len) != 0) {
        if (errno != EINPROGRESS && errno != EAGAIN) {
            close(fd);
            return -1;
        }
        if (wait_fd(fd, POLLOUT, deadline_ms) <= 0) {
            close(fd);
            return -1;
        }
    }
    return fd;
}

void omt_dbus_close(omt_dbus *bus) {
    if (bus->fd >= 0) close(bus->fd);
    bus->fd = -1;
    while (bus->pending_count) {
        omt_dbus_message_free(&bus->pending[bus->pending_head]);
        bus->pending_head = (bus->pending_head + 1) % OMT_DBUS_MAX_PENDING;
        bus->pending_count--;
    }
    omt_buf_free(&bus->inbox);
}

/* Reads the next complete message from the socket. */
static bool receive_message(omt_dbus *bus, uint64_t deadline_ms, omt_dbus_message *out,
                            omt_err *err) {
    for (;;) {
        if (bus->inbox.len >= 16) {
            size_t total = omt_dbus_message_length(bus->inbox.data, bus->inbox.len);
            if (total == 0) {
                omt_err_set(err, "malformed or oversized D-Bus message");
                return false;
            }
            if (bus->inbox.len >= total) {
                uint8_t *raw = malloc(total);
                if (!raw) {
                    omt_err_set(err, "out of memory");
                    return false;
                }
                memcpy(raw, bus->inbox.data, total);
                omt_buf_consume(&bus->inbox, total);
                if (!omt_dbus_parse(raw, total, out)) {
                    free(raw);
                    omt_err_set(err, "malformed D-Bus message");
                    return false;
                }
                return true;
            }
        }
        int rc = wait_fd(bus->fd, POLLIN, deadline_ms);
        if (rc == 0) {
            omt_err_set(err, "D-Bus deadline expired");
            return false;
        }
        uint8_t chunk[8192];
        ssize_t n = recv(bus->fd, chunk, sizeof(chunk), MSG_DONTWAIT);
        if (n == 0) {
            omt_err_set(err, "D-Bus connection closed");
            return false;
        }
        if (n < 0) {
            if (errno == EINTR || errno == EAGAIN) continue;
            omt_err_os(err, errno);
            return false;
        }
        omt_buf_append(&bus->inbox, chunk, (size_t)n);
        if (bus->inbox.failed) {
            omt_err_set(err, "D-Bus input exceeds its buffer");
            return false;
        }
    }
}

static void queue_pending(omt_dbus *bus, omt_dbus_message *m) {
    if (bus->pending_count == OMT_DBUS_MAX_PENDING) {
        /* Bounded: the oldest queued signal gives way, as a broadcast queue
         * with overflow does. */
        omt_dbus_message_free(&bus->pending[bus->pending_head]);
        bus->pending_head = (bus->pending_head + 1) % OMT_DBUS_MAX_PENDING;
        bus->pending_count--;
    }
    bus->pending[(bus->pending_head + bus->pending_count) % OMT_DBUS_MAX_PENDING] = *m;
    bus->pending_count++;
    memset(m, 0, sizeof(*m));
}

bool omt_dbus_next(omt_dbus *bus, uint64_t deadline_ms, omt_dbus_message *out, omt_err *err) {
    if (bus->pending_count) {
        *out = bus->pending[bus->pending_head];
        memset(&bus->pending[bus->pending_head], 0, sizeof(omt_dbus_message));
        bus->pending_head = (bus->pending_head + 1) % OMT_DBUS_MAX_PENDING;
        bus->pending_count--;
        return true;
    }
    return receive_message(bus, deadline_ms, out, err);
}

static bool send_call(omt_dbus *bus, uint8_t flags, const char *destination, const char *path,
                      const char *interface, const char *member, const omt_dbus_arg *args,
                      size_t argc, uint64_t deadline_ms, uint32_t *serial, omt_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, OMT_DBUS_MAX_MESSAGE);
    *serial = ++bus->serial;
    if (*serial == 0) *serial = ++bus->serial;
    bool ok =
        omt_dbus_build_call(&msg, *serial, flags, destination, path, interface, member, args, argc);
    if (!ok) omt_err_set(err, "unable to marshal the D-Bus call");
    if (ok) ok = send_all(bus->fd, msg.data, msg.len, deadline_ms, err);
    omt_buf_free(&msg);
    return ok;
}

bool omt_dbus_call(omt_dbus *bus, const char *destination, const char *path, const char *interface,
                   const char *member, const omt_dbus_arg *args, size_t argc, uint64_t deadline_ms,
                   omt_dbus_message *reply, omt_err *err) {
    uint32_t serial;
    if (!send_call(bus, 0, destination, path, interface, member, args, argc, deadline_ms, &serial,
                   err))
        return false;
    for (;;) {
        omt_dbus_message m;
        if (!receive_message(bus, deadline_ms, &m, err)) return false;
        if ((m.type == OMT_DBUS_METHOD_RETURN || m.type == OMT_DBUS_ERROR) &&
            m.reply_serial == serial) {
            if (m.type == OMT_DBUS_ERROR) {
                omt_err_set(err, "D-Bus call %s failed", member);
                omt_dbus_message_free(&m);
                return false;
            }
            *reply = m;
            return true;
        }
        if (m.type == OMT_DBUS_SIGNAL)
            queue_pending(bus, &m);
        else
            omt_dbus_message_free(&m);
    }
}

bool omt_dbus_send_no_reply(omt_dbus *bus, const char *destination, const char *path,
                            const char *interface, const char *member, uint64_t deadline_ms,
                            omt_err *err) {
    uint32_t serial;
    return send_call(bus, FLAG_NO_REPLY_EXPECTED, destination, path, interface, member, NULL, 0,
                     deadline_ms, &serial, err);
}

static bool authenticate(int fd, uint64_t deadline_ms, omt_err *err) {
    char uid[24], hex[64], line[512];
    snprintf(uid, sizeof(uid), "%u", (unsigned)getuid());
    size_t n = 0;
    for (size_t i = 0; uid[i] && n + 2 < sizeof(hex); i++)
        n += (size_t)snprintf(hex + n, sizeof(hex) - n, "%02x", (unsigned char)uid[i]);
    char auth[128];
    int len = snprintf(auth, sizeof(auth), "%cAUTH EXTERNAL %s\r\n", 0, hex);
    if (len <= 0 || !send_all(fd, (const uint8_t *)auth, (size_t)len, deadline_ms, err))
        return false;
    if (!read_line(fd, line, sizeof(line), deadline_ms, err)) return false;
    if (strncmp(line, "OK ", 3) != 0) {
        omt_err_set(err, "D-Bus authentication was refused");
        return false;
    }
    static const char begin[] = "BEGIN\r\n";
    return send_all(fd, (const uint8_t *)begin, sizeof(begin) - 1, deadline_ms, err);
}

bool omt_dbus_open_system(omt_dbus *bus, uint64_t deadline_ms, omt_err *err) {
    memset(bus, 0, sizeof(*bus));
    bus->fd = -1;
    omt_buf_init(&bus->inbox, OMT_DBUS_MAX_MESSAGE * 2);
    const char *address = getenv("DBUS_SYSTEM_BUS_ADDRESS");
    if (!address || !*address) address = DEFAULT_SYSTEM_BUS;
    /* An address may list alternatives separated by ';'. */
    const char *p = address;
    while (*p && bus->fd < 0) {
        const char *semi = strchr(p, ';');
        size_t len = semi ? (size_t)(semi - p) : strlen(p);
        bus->fd = connect_address(p, len, deadline_ms);
        p = semi ? semi + 1 : p + len;
    }
    if (bus->fd < 0) {
        omt_err_set(err, "unable to connect to the system bus");
        omt_dbus_close(bus);
        return false;
    }
    if (!authenticate(bus->fd, deadline_ms, err)) {
        omt_dbus_close(bus);
        return false;
    }
    omt_dbus_message reply;
    if (!omt_dbus_call(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                       "Hello", NULL, 0, deadline_ms, &reply, err)) {
        omt_dbus_close(bus);
        return false;
    }
    omt_dbus_message_free(&reply);
    return true;
}
