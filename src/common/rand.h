/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Operating-system randomness: getrandom(2) on Linux, BCryptGenRandom on
 * Windows. There is no fallback; a failure is reported, never papered over.
 */
#ifndef OMT_RAND_H
#define OMT_RAND_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

OMT_NODISCARD bool omt_random_bytes(void *out, size_t len, omt_err *err);
/* Appends 2*bytes lowercase hex characters of fresh randomness. */
OMT_NODISCARD bool omt_random_hex(omt_buf *out, size_t bytes, omt_err *err);

#endif
