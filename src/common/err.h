/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * A fixed-size error message. Functions that can fail take an omt_err * and
 * describe the failure in it; NULL is accepted where the caller only needs
 * the verdict.
 */
#ifndef OMT_ERR_H
#define OMT_ERR_H

#include "common/base.h"

typedef struct {
    char msg[256];
} omt_err;

void omt_err_set(omt_err *err, const char *fmt, ...) OMT_PRINTF(2, 3);
/* Sets the message the way Rust's io::Error displays an OS error:
 * "<strerror> (os error <n>)". */
void omt_err_os(omt_err *err, int code);
static inline void omt_err_clear(omt_err *err) {
    if (err) err->msg[0] = 0;
}

#endif
