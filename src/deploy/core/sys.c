/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The platform-neutral half of the deployer's system layer.
 */
#include "deploy/core/sys.h"

#include <stdarg.h>

void dp_err_init(dp_err *err) {
    if (err) omt_buf_init(&err->msg, DP_ERR_LIMIT);
}

void dp_err_free(dp_err *err) {
    if (err) omt_buf_free(&err->msg);
}

void dp_fail(dp_err *err, const char *fmt, ...) {
    if (!err) return;
    omt_buf_clear(&err->msg);
    err->msg.failed = false;
    va_list args;
    va_start(args, fmt);
    omt_buf_vprintf(&err->msg, fmt, args);
    va_end(args);
}

void dp_fail_cancelled(dp_err *err) { dp_fail(err, "operation cancelled"); }

void dp_path_join(omt_buf *out, const char *dir, const char *name) {
    omt_buf_puts(out, dir);
    size_t len = out->len;
    bool has_sep =
        len > 0 && (out->data[len - 1] == '/' || (DP_ON_WINDOWS && out->data[len - 1] == '\\'));
    if (len > 0 && !has_sep) omt_buf_putc(out, DP_ON_WINDOWS ? '\\' : '/');
    omt_buf_puts(out, name);
}
