/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include <errno.h>
#include <sys/random.h>

#include "common/rand.h"

bool omt_random_bytes(void *out, size_t len, omt_err *err) {
    uint8_t *p = out;
    while (len) {
        ssize_t n = getrandom(p, len, 0);
        if (n < 0) {
            if (errno == EINTR) continue;
            omt_err_os(err, errno);
            return false;
        }
        p += n;
        len -= (size_t)n;
    }
    return true;
}
