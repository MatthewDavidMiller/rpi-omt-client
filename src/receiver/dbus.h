/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * A minimal D-Bus client: enough of the wire protocol to call methods on the
 * system bus and read the signals sent back. It marshals only the types the
 * Avahi browse uses, and every incoming message is bounded and validated
 * before a field is read out of it.
 */
#ifndef OMT_RECEIVER_DBUS_H
#define OMT_RECEIVER_DBUS_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

/* Largest message accepted from the bus. Avahi's replies and signals are a
 * few hundred bytes; this leaves room for long TXT records. */
#define OMT_DBUS_MAX_MESSAGE (256u * 1024u)
/* Signals held while a method call waits for its reply. */
#define OMT_DBUS_MAX_PENDING 256u

enum {
    OMT_DBUS_METHOD_CALL = 1,
    OMT_DBUS_METHOD_RETURN = 2,
    OMT_DBUS_ERROR = 3,
    OMT_DBUS_SIGNAL = 4,
};

/* One received message. Header strings point into `raw`. */
typedef struct {
    uint8_t *raw;
    size_t len;
    bool big_endian;
    uint8_t type;
    uint32_t serial;
    uint32_t reply_serial;
    const char *path;
    const char *interface;
    const char *member;
    const char *signature;
    size_t body_offset;
    size_t body_len;
} omt_dbus_message;

void omt_dbus_message_free(omt_dbus_message *m);

/* A cursor over a message body. Every read checks bounds, alignment padding,
 * and string termination; any failure sets `failed` and later reads return
 * zero values. */
typedef struct {
    const omt_dbus_message *m;
    size_t pos; /* absolute offset into m->raw */
    size_t end;
    bool failed;
} omt_dbus_reader;

void omt_dbus_reader_init(omt_dbus_reader *r, const omt_dbus_message *m);
int32_t omt_dbus_read_i32(omt_dbus_reader *r);
uint32_t omt_dbus_read_u32(omt_dbus_reader *r);
uint16_t omt_dbus_read_u16(omt_dbus_reader *r);
bool omt_dbus_read_bool(omt_dbus_reader *r);
/* Strings and object paths; returns a NUL-terminated pointer into the message. */
const char *omt_dbus_read_string(omt_dbus_reader *r);
/* Skips an `aay` (an array of byte arrays). */
void omt_dbus_skip_aay(omt_dbus_reader *r);

/* An outgoing message under construction. */
typedef struct {
    omt_buf b;
} omt_dbus_writer;

typedef struct {
    int fd;
    uint32_t serial;
    omt_dbus_message pending[OMT_DBUS_MAX_PENDING];
    size_t pending_head;
    size_t pending_count;
    /* Bytes read past the last complete message. */
    omt_buf inbox;
} omt_dbus;

/* Connects to DBUS_SYSTEM_BUS_ADDRESS (or the default socket), authenticates
 * with EXTERNAL, and says Hello, all before the deadline. */
OMT_NODISCARD bool omt_dbus_open_system(omt_dbus *bus, uint64_t deadline_ms, omt_err *err);
void omt_dbus_close(omt_dbus *bus);

/* Body arguments for a call: a signature and a matching list of values. Only
 * 'i', 'u', 's', and 'o' are supported; each value is a pointer to an int32_t,
 * uint32_t, or NUL-terminated string. */
typedef struct {
    char type;
    union {
        int32_t i;
        uint32_t u;
        const char *s;
    } v;
} omt_dbus_arg;

/* Calls a method and waits until the deadline for its reply. Signals that
 * arrive meanwhile are queued for omt_dbus_next. An error reply fails the
 * call with its name. */
OMT_NODISCARD bool omt_dbus_call(omt_dbus *bus, const char *destination, const char *path,
                                 const char *interface, const char *member,
                                 const omt_dbus_arg *args, size_t argc, uint64_t deadline_ms,
                                 omt_dbus_message *reply, omt_err *err);
/* Sends a call flagged NO_REPLY_EXPECTED and does not wait. */
bool omt_dbus_send_no_reply(omt_dbus *bus, const char *destination, const char *path,
                            const char *interface, const char *member, uint64_t deadline_ms,
                            omt_err *err);
/* The next queued or incoming message, or false at the deadline / on error. */
OMT_NODISCARD bool omt_dbus_next(omt_dbus *bus, uint64_t deadline_ms, omt_dbus_message *out,
                                 omt_err *err);

/* Exposed for the fuzzer and the tests: parses one complete message. */
OMT_NODISCARD bool omt_dbus_parse(uint8_t *raw, size_t len, omt_dbus_message *out);
/* The total length of the message starting at `data` once 16 bytes are known,
 * or 0 when it is malformed or oversized. */
size_t omt_dbus_message_length(const uint8_t *data, size_t len);
/* Marshals a method call into `out` (exposed for tests). */
OMT_NODISCARD bool omt_dbus_build_call(omt_buf *out, uint32_t serial, uint8_t flags,
                                       const char *destination, const char *path,
                                       const char *interface, const char *member,
                                       const omt_dbus_arg *args, size_t argc);

#endif
