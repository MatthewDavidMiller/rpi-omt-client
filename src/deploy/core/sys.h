/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The deployer's operating-system layer. The deployer is the one program that
 * ships for both Linux and Windows, so everything that differs between them --
 * sockets, threads, files, processes, the environment, and the console -- is
 * behind this interface, with sys_posix.c and sys_win32.c as the two halves.
 * Paths and environment values are UTF-8 on both; the Windows half converts at
 * the boundary.
 */
#ifndef DP_SYS_H
#define DP_SYS_H

#include <stdatomic.h>

#include "common/base.h"
#include "common/buf.h"

/* ------------------------------------------------------------------ errors
 * Deployer errors carry an unbounded message: a failed remote step quotes the
 * remote output, which can be megabytes. The buffer is capped well above that. */
typedef struct {
    omt_buf msg;
} dp_err;

#define DP_ERR_LIMIT (16u * 1024u * 1024u)

void dp_err_init(dp_err *err);
void dp_err_free(dp_err *err);
/* Replaces the message. */
void dp_fail(dp_err *err, const char *fmt, ...) OMT_PRINTF(2, 3);
/* Sets "operation cancelled". */
void dp_fail_cancelled(dp_err *err);
static inline const char *dp_err_text(const dp_err *err) { return omt_buf_cstr(&err->msg); }
/* Sets "<what>: <OS error text>" for the calling thread's last OS error. */
void dp_fail_os(dp_err *err, const char *what);

/* A shared cancellation flag, set by the frontend and polled by the worker. */
typedef atomic_bool dp_cancel;
static inline bool dp_cancelled(const dp_cancel *flag) {
    return flag && atomic_load_explicit(flag, memory_order_relaxed);
}

/* ---------------------------------------------------------------- startup */
/* Process-wide setup: Winsock and the console code page on Windows. */
bool dp_sys_init(dp_err *err);

/* ----------------------------------------------------------------- time */
uint64_t dp_now_ms(void);
void dp_sleep_ms(uint32_t ms);

/* -------------------------------------------------------------- sockets */
typedef intptr_t dp_sock;
#define DP_SOCK_INVALID ((dp_sock) - 1)

/* Resolves `host` and connects to the first address that answers within
 * `timeout_ms` overall. */
bool dp_sock_connect(const char *host, uint16_t port, uint32_t timeout_ms, dp_sock *out,
                     dp_err *err);
/* Sends everything, waiting at most `timeout_ms` for the socket to drain. */
bool dp_sock_send(dp_sock s, const void *data, size_t len, uint32_t timeout_ms, dp_err *err);
/* Waits up to `timeout_ms` for data. Returns bytes read, 0 at end of stream,
 * -1 on error (err set), -2 when the wait expired. */
long dp_sock_recv(dp_sock s, void *buf, size_t len, uint32_t timeout_ms, dp_err *err);
/* Waits for readability (or writability). 1 ready, 0 timeout, -1 error. */
int dp_sock_wait(dp_sock s, bool write, uint32_t timeout_ms);
void dp_sock_close(dp_sock s);

/* --------------------------------------------------------------- threads */
typedef void (*dp_thread_fn)(void *arg);
/* Starts a detached thread with a generous stack. */
bool dp_thread_spawn(dp_thread_fn fn, void *arg, dp_err *err);

typedef struct dp_mutex dp_mutex;
dp_mutex *dp_mutex_new(void);
void dp_mutex_free(dp_mutex *m);
void dp_mutex_lock(dp_mutex *m);
void dp_mutex_unlock(dp_mutex *m);

/* ----------------------------------------------------------------- files */
typedef enum {
    DP_PATH_MISSING,
    DP_PATH_FILE,
    DP_PATH_DIR,
    DP_PATH_LINK, /* a symbolic link or Windows reparse point, never followed */
    DP_PATH_OTHER,
    DP_PATH_ERROR,
} dp_path_kind;

/* What `path` itself is, without following a final link. */
dp_path_kind dp_path_lstat(const char *path);
/* What `path` resolves to, following links. */
dp_path_kind dp_path_stat(const char *path);
bool dp_is_file(const char *path); /* follows links, as Path::is_file */
bool dp_is_dir(const char *path);
bool dp_is_executable_file(const char *path);
/* Size of a regular file, following links; false if it is not one. */
bool dp_file_size(const char *path, uint64_t *size);
/* The identity of a regular, non-link file: device, inode, size, and
 * modification time. Fails for anything else. */
bool dp_file_fingerprint(const char *path, omt_buf *out, dp_err *err);
/* Joins with the platform separator. */
void dp_path_join(omt_buf *out, const char *dir, const char *name);

typedef struct dp_file dp_file;
dp_file *dp_file_open_read(const char *path, dp_err *err);
/* Returns bytes read, 0 at end, -1 on error. */
long dp_file_read(dp_file *f, void *buf, size_t len, dp_err *err);
bool dp_file_rewind(dp_file *f, dp_err *err);
void dp_file_close(dp_file *f);
/* Reads a whole regular file of at most `limit` bytes. */
bool dp_read_file(const char *path, size_t limit, omt_buf *out, dp_err *err);
/* Creates or truncates `path`, writes `data`, and flushes it to the device. */
bool dp_write_file_sync(const char *path, const void *data, size_t len, dp_err *err);

/* ------------------------------------------------------------ environment */
/* The variable's UTF-8 value, or false when it is unset. */
bool dp_getenv(const char *name, omt_buf *out);
/* The current directory, or "." if it cannot be read. */
void dp_current_dir(omt_buf *out);
#ifdef _WIN32
#define DP_PATH_LIST_SEP ';'
#define DP_ON_WINDOWS true
#else
#define DP_PATH_LIST_SEP ':'
#define DP_ON_WINDOWS false
#endif

/* ------------------------------------------------------------ processes */
#define DP_OUTPUT_LIMIT (4u * 1024u * 1024u)

typedef struct {
    int exit_code;
    omt_buf output; /* stdout then stderr, lossily decoded, capped together */
} dp_process_result;

/* Runs `program` with `args` in `directory`, stdin closed, both streams
 * captured. `env` is NULL or "NAME=value" pairs added to the inherited
 * environment, NULL-terminated. Cancelling kills the whole process tree. */
bool dp_run_process(const char *program, const char *const *args, const char *directory,
                    const char *const *env, const dp_cancel *cancel, dp_process_result *result,
                    dp_err *err);
void dp_process_result_free(dp_process_result *result);

/* ------------------------------------------------------------- console */
/* Prompts on the controlling terminal and reads a line with echo off. The
 * line ending is removed. */
bool dp_prompt_secret(const char *prompt, omt_buf *out, dp_err *err);
/* Reads standard input to end of stream, failing once more than `limit`
 * bytes arrive. */
bool dp_read_stdin(size_t limit, omt_buf *out, bool *too_long, dp_err *err);

#endif
