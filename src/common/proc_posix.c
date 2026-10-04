/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "common/err.h"
#include "common/json.h"
#include "common/proc.h"

extern char **environ;

/* GCC's descriptor-leak analysis loses track of descriptors held in the pipe
 * arrays across the spawn path's shared cleanup, and reports every one of them
 * as leaked on paths where close_fd has closed it. The sanitizer suites and
 * test_proc's descriptor accounting cover this file instead. */
#if defined(__GNUC__) && !defined(__clang__)
#pragma GCC diagnostic ignored "-Wanalyzer-fd-leak"
#endif

/* posix_spawn takes `char *const[]` for historical reasons and never writes
 * through it; this is the one place the qualifier is dropped. */
static char *const *spawn_vector(const char *const *v) {
    union {
        const char *const *in;
        char *const *out;
    } u = {v};
    return u.out;
}

uint64_t omt_now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000u + (uint64_t)ts.tv_nsec / 1000000u;
}

double omt_now_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

uint64_t omt_remaining_ms(uint64_t deadline_ms) {
    uint64_t now = omt_now_ms();
    return deadline_ms > now ? deadline_ms - now : 0;
}

typedef struct {
    int fd;
    omt_buf bytes;
    size_t limit;
    bool truncated;
    bool eof;
} capture;

/* Reads what is queued, at most 16 chunks per turn so one busy stream cannot
 * starve the other or the deadline. Returns -1 on error, 1 when the budget ran
 * out with more pending, and 0 otherwise. */
static int drain(capture *c) {
    uint8_t chunk[8192];
    for (int turn = 0; turn < 16; turn++) {
        if (c->eof) return 0;
        ssize_t n = read(c->fd, chunk, sizeof(chunk));
        if (n == 0) {
            c->eof = true;
            return 0;
        }
        if (n < 0) {
            if (errno == EINTR) continue;
            if (errno == EAGAIN || errno == EWOULDBLOCK) return 0;
            return -1;
        }
        size_t room = c->limit - c->bytes.len;
        size_t kept = omt_min_size((size_t)n, room);
        omt_buf_append(&c->bytes, chunk, kept);
        if (kept < (size_t)n) c->truncated = true;
    }
    return 1;
}

static void set_nonblocking(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags >= 0) fcntl(fd, F_SETFL, flags | O_NONBLOCK);
}

static void close_fd(int *fd) {
    if (*fd >= 0) close(*fd);
    *fd = -1;
}

static void join_command(const char *const *argv, omt_proc_result *r) {
    omt_buf b;
    omt_buf_init(&b, 64 * 1024);
    for (size_t i = 0; argv[i]; i++) {
        if (i) omt_buf_putc(&b, ' ');
        omt_buf_puts(&b, argv[i]);
    }
    r->command = omt_buf_take(&b, NULL);
}

void omt_proc_result_free(omt_proc_result *r) {
    free(r->command);
    omt_buf_free(&r->out);
    omt_buf_free(&r->err);
    memset(r, 0, sizeof(*r));
}

void omt_proc_run(const omt_proc_options *o, omt_proc_result *r) {
    memset(r, 0, sizeof(*r));
    size_t limit = o->output_limit ? o->output_limit : OMT_PROC_OUTPUT_LIMIT;
    omt_buf_init(&r->out, limit * 3 + 16);
    omt_buf_init(&r->err, limit * 3 + 16);
    join_command(o->argv, r);
    double started = omt_now_seconds();
    uint64_t deadline = omt_now_ms() + o->timeout_ms;

    int out_pipe[2] = {-1, -1}, err_pipe[2] = {-1, -1}, in_pipe[2] = {-1, -1};
    int devnull = -1;
    pid_t pid = -1;
    posix_spawn_file_actions_t actions;
    posix_spawnattr_t attr;
    bool actions_ready = false, attr_ready = false;
    int spawn_error = 0;

    if (pipe2(out_pipe, O_CLOEXEC) != 0) {
        spawn_error = errno;
        out_pipe[0] = out_pipe[1] = -1;
        goto spawn_failed;
    }
    if (pipe2(err_pipe, O_CLOEXEC) != 0) {
        spawn_error = errno;
        err_pipe[0] = err_pipe[1] = -1;
        goto spawn_failed;
    }
    if (o->stdin_data) {
        if (pipe2(in_pipe, O_CLOEXEC) != 0) {
            spawn_error = errno;
            in_pipe[0] = in_pipe[1] = -1;
            goto spawn_failed;
        }
    } else {
        devnull = open("/dev/null", O_RDONLY | O_CLOEXEC);
        if (devnull < 0) {
            spawn_error = errno;
            goto spawn_failed;
        }
    }
    if ((spawn_error = posix_spawn_file_actions_init(&actions))) goto spawn_failed;
    actions_ready = true;
    posix_spawn_file_actions_adddup2(&actions, o->stdin_data ? in_pipe[0] : devnull, 0);
    posix_spawn_file_actions_adddup2(&actions, out_pipe[1], 1);
    posix_spawn_file_actions_adddup2(&actions, err_pipe[1], 2);
    if ((spawn_error = posix_spawnattr_init(&attr))) goto spawn_failed;
    attr_ready = true;
    sigset_t defaults;
    sigemptyset(&defaults);
    sigaddset(&defaults, SIGPIPE);
    sigaddset(&defaults, SIGINT);
    sigaddset(&defaults, SIGTERM);
    sigset_t none;
    sigemptyset(&none);
    posix_spawnattr_setsigdefault(&attr, &defaults);
    posix_spawnattr_setsigmask(&attr, &none);
    posix_spawnattr_setpgroup(&attr, 0);
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETPGROUP | POSIX_SPAWN_SETSIGDEF |
                                        POSIX_SPAWN_SETSIGMASK);
    spawn_error = posix_spawn(&pid, o->argv[0], &actions, &attr, spawn_vector(o->argv),
                              o->envp ? spawn_vector(o->envp) : environ);
spawn_failed:
    if (actions_ready) posix_spawn_file_actions_destroy(&actions);
    if (attr_ready) posix_spawnattr_destroy(&attr);
    /* The child's ends belong to the child now (or to nobody). */
    close_fd(&devnull);
    close_fd(&out_pipe[1]);
    close_fd(&err_pipe[1]);
    close_fd(&in_pipe[0]);
    if (spawn_error) {
        close_fd(&out_pipe[0]);
        close_fd(&err_pipe[0]);
        close_fd(&in_pipe[1]);
        omt_err e;
        omt_err_os(&e, spawn_error);
        omt_strlcpy(r->error, e.msg, sizeof(r->error));
        r->duration_seconds = omt_now_seconds() - started;
        return;
    }

    capture out = {out_pipe[0], {0}, limit, false, false};
    capture err = {err_pipe[0], {0}, limit, false, false};
    omt_buf_init(&out.bytes, limit);
    omt_buf_init(&err.bytes, limit);
    set_nonblocking(out.fd);
    set_nonblocking(err.fd);
    int in_fd = in_pipe[1];
    size_t in_written = 0;
    if (in_fd >= 0) set_nonblocking(in_fd);

    bool exited = false, failed = false, timed_out = false;
    int status = 0;
    int io_error = 0;
    for (;;) {
        int a = drain(&out), b = drain(&err);
        if (a < 0 || b < 0) {
            io_error = errno;
            failed = true;
            break;
        }
        if (in_fd >= 0) {
            if (in_written < o->stdin_len) {
                ssize_t n = write(in_fd, (const uint8_t *)o->stdin_data + in_written,
                                  o->stdin_len - in_written);
                if (n > 0)
                    in_written += (size_t)n;
                else if (n < 0 && errno != EAGAIN && errno != EINTR)
                    in_written = o->stdin_len;
            }
            if (in_written >= o->stdin_len) {
                close(in_fd);
                in_fd = -1;
            }
        }
        if (!exited) {
            pid_t w = waitpid(pid, &status, WNOHANG);
            if (w == pid)
                exited = true;
            else if (w < 0 && errno != EINTR) {
                io_error = errno;
                failed = true;
                break;
            }
        }
        if (exited && out.eof && err.eof) break;
        uint64_t now = omt_now_ms();
        if (now >= deadline) {
            timed_out = true;
            break;
        }
        if (a == 0 && b == 0) {
            /* Sleep until a pipe has data or 10 ms pass, whichever is first;
             * the short cap is what notices the child's exit. */
            struct pollfd fds[3];
            nfds_t n = 0;
            if (!out.eof) fds[n++] = (struct pollfd){out.fd, POLLIN, 0};
            if (!err.eof) fds[n++] = (struct pollfd){err.fd, POLLIN, 0};
            if (in_fd >= 0) fds[n++] = (struct pollfd){in_fd, POLLOUT, 0};
            uint64_t wait = omt_min_size((size_t)(deadline - now), 10);
            if (n)
                poll(fds, n, (int)wait);
            else
                nanosleep(&(struct timespec){0, (long)wait * 1000000L}, NULL);
        }
    }
    if (timed_out || failed) {
        kill(-pid, SIGKILL);
        if (!exited) {
            kill(pid, SIGKILL);
            while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
            exited = true;
        }
    }
    if (in_fd >= 0) close(in_fd);
    close(out.fd);
    close(err.fd);

    if (!timed_out && !failed && WIFEXITED(status)) {
        r->has_returncode = true;
        r->returncode = WEXITSTATUS(status);
    }
    omt_utf8_lossy(&r->out, omt_buf_cstr(&out.bytes), out.bytes.len);
    omt_utf8_lossy(&r->err, omt_buf_cstr(&err.bytes), err.bytes.len);
    omt_buf_append(&r->out, "", 0);
    omt_buf_append(&r->err, "", 0);
    omt_buf_free(&out.bytes);
    omt_buf_free(&err.bytes);
    r->stdout_truncated = out.truncated;
    r->stderr_truncated = err.truncated;
    r->timed_out = timed_out;
    r->duration_seconds = omt_now_seconds() - started;
    if (timed_out) {
        omt_buf secs;
        omt_buf_init(&secs, 64);
        omt_fmt_f64_display(&secs, (double)o->timeout_ms / 1000.0);
        snprintf(r->error, sizeof(r->error), "Command exceeded %s seconds.", omt_buf_cstr(&secs));
        omt_buf_free(&secs);
    } else if (failed) {
        omt_err e;
        omt_err_os(&e, io_error);
        omt_strlcpy(r->error, e.msg, sizeof(r->error));
    }
}

static const char *trimmed(const omt_buf *b, omt_buf *scratch) {
    omt_span s = omt_utf8_trim(omt_buf_cstr(b), b->len);
    omt_buf_clear(scratch);
    omt_buf_append(scratch, s.p, s.len);
    omt_buf_append(scratch, "", 0);
    return omt_buf_cstr(scratch);
}

const char *omt_proc_failure_detail(const omt_proc_result *r, omt_buf *scratch) {
    if (r->error[0]) return r->error;
    const char *err = trimmed(&r->err, scratch);
    if (*err) return err;
    return trimmed(&r->out, scratch);
}

const char *omt_proc_report_text(const omt_proc_result *r, omt_buf *scratch) {
    const char *out = trimmed(&r->out, scratch);
    if (*out) return out;
    if (r->error[0]) return r->error;
    const char *err = trimmed(&r->err, scratch);
    if (*err) return err;
    return "unavailable";
}
