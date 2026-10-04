/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/err.h"

#include <stdarg.h>
#include <stdio.h>
#include <string.h>

void omt_err_set(omt_err *err, const char *fmt, ...) {
    if (!err) return;
    va_list args;
    va_start(args, fmt);
    vsnprintf(err->msg, sizeof(err->msg), fmt, args);
    va_end(args);
}

void omt_err_os(omt_err *err, int code) {
    if (!err) return;
    char text[160] = "unknown error";
#ifdef _WIN32
    strerror_s(text, sizeof(text), code);
#else
    /* GNU and XSI strerror_r differ; strerror_l-free code reads either. */
#if defined(__GLIBC__) && defined(_GNU_SOURCE)
    const char *s = strerror_r(code, text, sizeof(text));
    if (s != text) snprintf(text, sizeof(text), "%s", s);
#else
    if (strerror_r(code, text, sizeof(text)) != 0) snprintf(text, sizeof(text), "error %d", code);
#endif
#endif
    omt_err_set(err, "%s (os error %d)", text, code);
}
