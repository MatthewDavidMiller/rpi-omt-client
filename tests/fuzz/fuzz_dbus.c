/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * D-Bus message parsing and the body reads the Avahi browse performs.
 */
#include <stdlib.h>
#include <string.h>

#include "receiver/dbus.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    size_t total = omt_dbus_message_length(data, size);
    if (total == 0 || total > size) return 0;
    uint8_t *raw = malloc(total);
    if (!raw) return 0;
    memcpy(raw, data, total);
    omt_dbus_message m;
    if (!omt_dbus_parse(raw, total, &m)) {
        free(raw);
        return 0;
    }
    omt_dbus_reader r;
    omt_dbus_reader_init(&r, &m);
    (void)omt_dbus_read_i32(&r);
    (void)omt_dbus_read_i32(&r);
    (void)omt_dbus_read_string(&r);
    (void)omt_dbus_read_string(&r);
    (void)omt_dbus_read_u16(&r);
    omt_dbus_skip_aay(&r);
    (void)omt_dbus_read_bool(&r);
    omt_dbus_message_free(&m);
    return 0;
}
