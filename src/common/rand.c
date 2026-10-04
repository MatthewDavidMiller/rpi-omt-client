/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/rand.h"

#include <string.h>

bool omt_random_hex(omt_buf *out, size_t bytes, omt_err *err) {
    uint8_t chunk[64];
    while (bytes) {
        size_t n = omt_min_size(bytes, sizeof(chunk));
        if (!omt_random_bytes(chunk, n, err)) return false;
        omt_buf_hex(out, chunk, n);
        bytes -= n;
    }
    memset(chunk, 0, sizeof(chunk));
    if (out->failed) {
        omt_err_set(err, "random value exceeds its buffer");
        return false;
    }
    return true;
}
