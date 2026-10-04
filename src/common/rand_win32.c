/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
/* windows.h first: bcrypt.h uses its base types. */
#include <windows.h>

#include <bcrypt.h>

#include "common/rand.h"

bool omt_random_bytes(void *out, size_t len, omt_err *err) {
    uint8_t *p = out;
    while (len) {
        ULONG chunk = len > 0x10000000u ? 0x10000000u : (ULONG)len;
        NTSTATUS status = BCryptGenRandom(NULL, p, chunk, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
        if (status != 0) {
            omt_err_set(err, "BCryptGenRandom failed (0x%08lx)", (unsigned long)status);
            return false;
        }
        p += chunk;
        len -= chunk;
    }
    return true;
}
