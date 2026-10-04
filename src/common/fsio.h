/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * File access for paths an operator or another process may control.
 *
 * Reads refuse symlinks and non-regular files, open with O_NOFOLLOW, and
 * confirm the opened file is the one that was checked (same device and inode)
 * before reading at most a stated number of bytes. Replacements stage into a
 * fresh mode-0600 file beside the target, fsync it, rename it over, and fsync
 * the directory, so a crash leaves either the old document or the new one.
 */
#ifndef OMT_FSIO_H
#define OMT_FSIO_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

typedef enum {
    OMT_READ_ERROR = -1,
    OMT_READ_MISSING = 0,
    OMT_READ_OK = 1,
} omt_read_result;

/* Reads the whole file into `out` (which this initializes). */
omt_read_result omt_read_bounded(const char *path, size_t maximum, omt_buf *out, omt_err *err);
/* As omt_read_bounded, and also requires valid UTF-8. */
omt_read_result omt_read_text(const char *path, size_t maximum, omt_buf *out, omt_err *err);
OMT_NODISCARD bool omt_atomic_replace(const char *path, const void *data, size_t len,
                                      size_t maximum, omt_err *err);
OMT_NODISCARD bool omt_remove_file_durable(const char *path, omt_err *err);
/* Rewrites an existing regular file in place, keeping its inode -- for
 * request files another process watches by inode. */
OMT_NODISCARD bool omt_write_fixed_inode(const char *path, const void *data, size_t len,
                                         size_t maximum, omt_err *err);

/* mkdir -p with `mode` for each directory created (subject to the umask). */
OMT_NODISCARD bool omt_mkdir_all(const char *path, unsigned mode, omt_err *err);

#endif
