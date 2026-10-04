/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The POSIX half of the deployer's system layer.
 */
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#include "common/err.h"
#include "common/json.h"
#include "deploy/core/sys.h"

extern char **environ;

/* The descriptor-leak analysis cannot follow descriptors through the pipe
 * arrays' shared cleanup; see the same note in common/proc_posix.c. */
#if defined(__GNUC__) && !defined(__clang__)
#pragma GCC diagnostic ignored "-Wanalyzer-fd-leak"
#endif

void dp_fail_os(dp_err *err, const char *what) {
    omt_err os;
    omt_err_os(&os, errno);
    dp_fail(err, "%s: %s", what, os.msg);
}

bool dp_sys_init(dp_err *err) {
    (void)err;
    /* A peer that closes mid-write must be an error return, not a signal. */
    signal(SIGPIPE, SIG_IGN);
    return true;
}

uint64_t dp_now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000u + (uint64_t)ts.tv_nsec / 1000000u;
}

void dp_sleep_ms(uint32_t ms) {
    struct timespec ts = {(time_t)(ms / 1000u), (long)(ms % 1000u) * 1000000L};
    while (nanosleep(&ts, &ts) != 0 && errno == EINTR) {}
}

/* -------------------------------------------------------------- sockets */

static int poll_one(int fd, short events, uint32_t timeout_ms) {
    struct pollfd p = {fd, events, 0};
    for (;;) {
        int rc = poll(&p, 1, timeout_ms > (uint32_t)INT32_MAX ? INT32_MAX : (int)timeout_ms);
        if (rc < 0 && errno == EINTR) continue;
        return rc;
    }
}

bool dp_sock_connect(const char *host, uint16_t port, uint32_t timeout_ms, dp_sock *out,
                     dp_err *err) {
    *out = DP_SOCK_INVALID;
    char service[8];
    omt_snprintf(service, sizeof(service), "%u", (unsigned)port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_ADDRCONFIG;
    struct addrinfo *list = NULL;
    int rc = getaddrinfo(host, service, &hints, &list);
    if (rc != 0) {
        dp_fail(err, "cannot resolve %s: %s", host, gai_strerror(rc));
        return false;
    }
    uint64_t deadline = dp_now_ms() + timeout_ms;
    int saved = ETIMEDOUT;
    for (struct addrinfo *ai = list; ai; ai = ai->ai_next) {
        uint64_t now = dp_now_ms();
        if (now >= deadline) break;
        int fd =
            socket(ai->ai_family, ai->ai_socktype | SOCK_CLOEXEC | SOCK_NONBLOCK, ai->ai_protocol);
        if (fd < 0) {
            saved = errno;
            continue;
        }
        if (connect(fd, ai->ai_addr, ai->ai_addrlen) != 0) {
            if (errno != EINPROGRESS) {
                saved = errno;
                close(fd);
                continue;
            }
            int ready = poll_one(fd, POLLOUT, (uint32_t)(deadline - now));
            int soerr = 0;
            socklen_t sl = sizeof(soerr);
            if (ready <= 0) {
                saved = ready == 0 ? ETIMEDOUT : errno;
                close(fd);
                continue;
            }
            if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &soerr, &sl) != 0 || soerr != 0) {
                saved = soerr ? soerr : errno;
                close(fd);
                continue;
            }
        }
        int one = 1;
        setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
        freeaddrinfo(list);
        *out = fd;
        return true;
    }
    freeaddrinfo(list);
    errno = saved;
    if (saved == ETIMEDOUT) {
        dp_fail(err, "connecting to %s:%u timed out", host, (unsigned)port);
    } else {
        char what[300];
        omt_snprintf(what, sizeof(what), "cannot connect to %s:%u", host, (unsigned)port);
        dp_fail_os(err, what);
    }
    return false;
}

bool dp_sock_send(dp_sock s, const void *data, size_t len, uint32_t timeout_ms, dp_err *err) {
    const uint8_t *p = data;
    uint64_t deadline = dp_now_ms() + timeout_ms;
    while (len > 0) {
        ssize_t n = send((int)s, p, len, MSG_NOSIGNAL);
        if (n > 0) {
            p += n;
            len -= (size_t)n;
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            uint64_t now = dp_now_ms();
            if (now >= deadline || poll_one((int)s, POLLOUT, (uint32_t)(deadline - now)) == 0) {
                dp_fail(err, "network write timed out");
                return false;
            }
            continue;
        }
        dp_fail_os(err, "network write failed");
        return false;
    }
    return true;
}

long dp_sock_recv(dp_sock s, void *buf, size_t len, uint32_t timeout_ms, dp_err *err) {
    for (;;) {
        ssize_t n = recv((int)s, buf, len, 0);
        if (n >= 0) return (long)n;
        if (errno == EINTR) continue;
        if (errno != EAGAIN && errno != EWOULDBLOCK) {
            dp_fail_os(err, "network read failed");
            return -1;
        }
        int ready = poll_one((int)s, POLLIN, timeout_ms);
        if (ready == 0) return -2;
        if (ready < 0) {
            dp_fail_os(err, "network wait failed");
            return -1;
        }
    }
}

int dp_sock_wait(dp_sock s, bool write, uint32_t timeout_ms) {
    int rc = poll_one((int)s, write ? POLLOUT : POLLIN, timeout_ms);
    return rc > 0 ? 1 : rc;
}

void dp_sock_close(dp_sock s) {
    if (s != DP_SOCK_INVALID) close((int)s);
}

/* --------------------------------------------------------------- threads */

typedef struct {
    dp_thread_fn fn;
    void *arg;
} thread_start;

static void *thread_main(void *raw) {
    thread_start start = *(thread_start *)raw;
    free(raw);
    start.fn(start.arg);
    return NULL;
}

bool dp_thread_spawn(dp_thread_fn fn, void *arg, dp_err *err) {
    thread_start *start = malloc(sizeof(*start));
    if (!start) {
        dp_fail(err, "out of memory");
        return false;
    }
    start->fn = fn;
    start->arg = arg;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    /* musl's default thread stack is 128 KiB; key derivation and the SSH
     * packet buffers want more headroom than that. */
    pthread_attr_setstacksize(&attr, 2u * 1024u * 1024u);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    pthread_t thread;
    int rc = pthread_create(&thread, &attr, thread_main, start);
    pthread_attr_destroy(&attr);
    if (rc != 0) {
        free(start);
        errno = rc;
        dp_fail_os(err, "cannot start a worker thread");
        return false;
    }
    return true;
}

struct dp_mutex {
    pthread_mutex_t m;
};

dp_mutex *dp_mutex_new(void) {
    dp_mutex *m = malloc(sizeof(*m));
    if (m) pthread_mutex_init(&m->m, NULL);
    return m;
}
void dp_mutex_free(dp_mutex *m) {
    if (!m) return;
    pthread_mutex_destroy(&m->m);
    free(m);
}
void dp_mutex_lock(dp_mutex *m) { pthread_mutex_lock(&m->m); }
void dp_mutex_unlock(dp_mutex *m) { pthread_mutex_unlock(&m->m); }

/* ----------------------------------------------------------------- files */

static dp_path_kind kind_of(const struct stat *st) {
    if (S_ISREG(st->st_mode)) return DP_PATH_FILE;
    if (S_ISDIR(st->st_mode)) return DP_PATH_DIR;
    if (S_ISLNK(st->st_mode)) return DP_PATH_LINK;
    return DP_PATH_OTHER;
}

dp_path_kind dp_path_lstat(const char *path) {
    struct stat st;
    if (lstat(path, &st) != 0)
        return errno == ENOENT || errno == ENOTDIR ? DP_PATH_MISSING : DP_PATH_ERROR;
    return kind_of(&st);
}

dp_path_kind dp_path_stat(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0)
        return errno == ENOENT || errno == ENOTDIR ? DP_PATH_MISSING : DP_PATH_ERROR;
    return kind_of(&st);
}

bool dp_is_file(const char *path) { return dp_path_stat(path) == DP_PATH_FILE; }
bool dp_is_dir(const char *path) { return dp_path_stat(path) == DP_PATH_DIR; }

bool dp_is_executable_file(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 && S_ISREG(st.st_mode) && (st.st_mode & 0111) != 0;
}

bool dp_file_size(const char *path, uint64_t *size) {
    struct stat st;
    if (stat(path, &st) != 0 || !S_ISREG(st.st_mode)) return false;
    *size = (uint64_t)st.st_size;
    return true;
}

bool dp_file_fingerprint(const char *path, omt_buf *out, dp_err *err) {
    struct stat st;
    if (lstat(path, &st) != 0 || !S_ISREG(st.st_mode)) {
        dp_fail(err, "deployment artifact is missing or unsafe: %s", path);
        return false;
    }
    uint64_t nanos = (uint64_t)st.st_mtim.tv_sec * 1000000000u + (uint64_t)st.st_mtim.tv_nsec;
    omt_buf_printf(out, "%llu:%llu:%llu:%llu", (unsigned long long)st.st_dev,
                   (unsigned long long)st.st_ino, (unsigned long long)st.st_size,
                   (unsigned long long)nanos);
    return !out->failed;
}

struct dp_file {
    int fd;
};

dp_file *dp_file_open_read(const char *path, dp_err *err) {
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        dp_fail_os(err, path);
        return NULL;
    }
    dp_file *f = malloc(sizeof(*f));
    if (!f) {
        close(fd);
        dp_fail(err, "out of memory");
        return NULL;
    }
    f->fd = fd;
    return f;
}

long dp_file_read(dp_file *f, void *buf, size_t len, dp_err *err) {
    for (;;) {
        ssize_t n = read(f->fd, buf, len);
        if (n >= 0) return (long)n;
        if (errno == EINTR) continue;
        dp_fail_os(err, "read failed");
        return -1;
    }
}

bool dp_file_rewind(dp_file *f, dp_err *err) {
    if (lseek(f->fd, 0, SEEK_SET) == 0) return true;
    dp_fail_os(err, "seek failed");
    return false;
}

void dp_file_close(dp_file *f) {
    if (!f) return;
    close(f->fd);
    free(f);
}

bool dp_read_file(const char *path, size_t limit, omt_buf *out, dp_err *err) {
    dp_file *f = dp_file_open_read(path, err);
    if (!f) return false;
    uint8_t chunk[16384];
    bool ok = true;
    for (;;) {
        long n = dp_file_read(f, chunk, sizeof(chunk), err);
        if (n < 0) {
            ok = false;
            break;
        }
        if (n == 0) break;
        if (out->len + (size_t)n > limit) {
            dp_fail(err, "%s is larger than %zu bytes", path, limit);
            ok = false;
            break;
        }
        omt_buf_append(out, chunk, (size_t)n);
    }
    dp_file_close(f);
    if (ok && out->failed) {
        dp_fail(err, "out of memory reading %s", path);
        ok = false;
    }
    return ok;
}

bool dp_write_file_sync(const char *path, const void *data, size_t len, dp_err *err) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC | O_NOFOLLOW, 0644);
    if (fd < 0) {
        dp_fail_os(err, path);
        return false;
    }
    const uint8_t *p = data;
    while (len > 0) {
        ssize_t n = write(fd, p, len);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) {
            dp_fail_os(err, path);
            close(fd);
            return false;
        }
        p += n;
        len -= (size_t)n;
    }
    if (fsync(fd) != 0) {
        dp_fail_os(err, path);
        close(fd);
        return false;
    }
    if (close(fd) != 0) {
        dp_fail_os(err, path);
        return false;
    }
    return true;
}

/* ------------------------------------------------------------ environment */

bool dp_getenv(const char *name, omt_buf *out) {
    const char *value = getenv(name);
    if (!value) return false;
    omt_buf_puts(out, value);
    return true;
}

void dp_current_dir(omt_buf *out) {
    char path[4096];
    omt_buf_puts(out, getcwd(path, sizeof(path)) ? path : ".");
}

/* ------------------------------------------------------------ processes */

static char *const *spawn_vector(const char *const *v) {
    union {
        const char *const *in;
        char *const *out;
    } u = {v};
    return u.out;
}

void dp_process_result_free(dp_process_result *result) { omt_buf_free(&result->output); }

/* Appends what is readable on fd without passing the shared limit. Returns
 * false once the stream has ended. */
static bool pump(int fd, omt_buf *into) {
    uint8_t chunk[8192];
    for (;;) {
        ssize_t n = read(fd, chunk, sizeof(chunk));
        if (n > 0) {
            size_t room = DP_OUTPUT_LIMIT > into->len ? DP_OUTPUT_LIMIT - into->len : 0;
            omt_buf_append(into, chunk, omt_min_size((size_t)n, room));
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) return true;
        /* End of stream, or an error: either way the stream is done, and what
         * was read so far is kept. */
        return false;
    }
}

bool dp_run_process(const char *program, const char *const *args, const char *directory,
                    const char *const *env, const dp_cancel *cancel, dp_process_result *result,
                    dp_err *err) {
    omt_buf_init(&result->output, DP_OUTPUT_LIMIT + 1);
    result->exit_code = 1;
    if (dp_cancelled(cancel)) {
        dp_fail_cancelled(err);
        return false;
    }
    size_t argc = 0;
    while (args && args[argc]) argc++;
    const char **argv = calloc(argc + 2, sizeof(*argv));
    size_t envc = 0;
    while (environ[envc]) envc++;
    size_t extra = 0;
    while (env && env[extra]) extra++;
    const char **envp = calloc(envc + extra + 1, sizeof(*envp));
    if (!argv || !envp) {
        free(argv);
        free(envp);
        dp_fail(err, "out of memory");
        return false;
    }
    argv[0] = program;
    for (size_t i = 0; i < argc; i++) argv[i + 1] = args[i];
    size_t n = 0;
    for (size_t i = 0; i < envc; i++) {
        /* An added variable replaces an inherited one of the same name. */
        const char *eq = strchr(environ[i], '=');
        size_t name_len = eq ? (size_t)(eq - environ[i]) : strlen(environ[i]);
        bool replaced = false;
        for (size_t j = 0; j < extra; j++) {
            if (strncmp(env[j], environ[i], name_len) == 0 && env[j][name_len] == '=')
                replaced = true;
        }
        if (!replaced) envp[n++] = environ[i];
    }
    for (size_t j = 0; j < extra; j++) envp[n++] = env[j];

    int out_pipe[2] = {-1, -1}, err_pipe[2] = {-1, -1};
    bool ok = false;
    pid_t pid = -1;
    posix_spawn_file_actions_t actions;
    posix_spawnattr_t attr;
    bool actions_ready = false, attr_ready = false;
    if (pipe2(out_pipe, O_CLOEXEC) != 0 || pipe2(err_pipe, O_CLOEXEC) != 0) {
        dp_fail_os(err, "cannot create a pipe");
        goto done;
    }
    if (posix_spawn_file_actions_init(&actions) != 0) goto spawn_failed;
    actions_ready = true;
    if (posix_spawnattr_init(&attr) != 0) goto spawn_failed;
    attr_ready = true;
    posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0);
    posix_spawn_file_actions_adddup2(&actions, out_pipe[1], 1);
    posix_spawn_file_actions_adddup2(&actions, err_pipe[1], 2);
    if (directory && posix_spawn_file_actions_addchdir_np(&actions, directory) != 0)
        goto spawn_failed;
    /* Its own process group, so cancelling stops descendants too. */
    posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETPGROUP);
    posix_spawnattr_setpgroup(&attr, 0);
    int rc = posix_spawn(&pid, program, &actions, &attr, spawn_vector(argv), spawn_vector(envp));
    if (rc != 0) {
        errno = rc;
        char what[4200];
        omt_snprintf(what, sizeof(what), "cannot run %s", program);
        dp_fail_os(err, what);
        pid = -1;
        goto done;
    }
    close(out_pipe[1]);
    close(err_pipe[1]);
    out_pipe[1] = err_pipe[1] = -1;
    fcntl(out_pipe[0], F_SETFL, O_NONBLOCK);
    fcntl(err_pipe[0], F_SETFL, O_NONBLOCK);

    omt_buf out_bytes, err_bytes;
    omt_buf_init(&out_bytes, DP_OUTPUT_LIMIT + 1);
    omt_buf_init(&err_bytes, DP_OUTPUT_LIMIT + 1);
    bool out_open = true, err_open = true, exited = false;
    int status = 0;
    for (;;) {
        if (dp_cancelled(cancel)) {
            kill(-pid, SIGKILL);
            waitpid(pid, NULL, 0);
            pid = -1;
            omt_buf_free(&out_bytes);
            omt_buf_free(&err_bytes);
            dp_fail_cancelled(err);
            goto done;
        }
        if (!exited) {
            pid_t w = waitpid(pid, &status, WNOHANG);
            if (w == pid) exited = true;
        }
        if (exited && !out_open && !err_open) break;
        struct pollfd fds[2];
        nfds_t count = 0;
        if (out_open) fds[count++] = (struct pollfd){out_pipe[0], POLLIN, 0};
        if (err_open) fds[count++] = (struct pollfd){err_pipe[0], POLLIN, 0};
        if (count) {
            poll(fds, count, 25);
        } else {
            dp_sleep_ms(25);
        }
        if (out_open) out_open = pump(out_pipe[0], &out_bytes);
        if (err_open) err_open = pump(err_pipe[0], &err_bytes);
    }
    pid = -1;
    {
        /* stdout, then as much of stderr as still fits, as one text. */
        size_t room = DP_OUTPUT_LIMIT > out_bytes.len ? DP_OUTPUT_LIMIT - out_bytes.len : 0;
        omt_buf_append(&out_bytes, err_bytes.data, omt_min_size(err_bytes.len, room));
        omt_utf8_lossy(&result->output, (const char *)out_bytes.data, out_bytes.len);
        omt_buf_free(&out_bytes);
        omt_buf_free(&err_bytes);
        result->exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
        ok = !result->output.failed;
        if (!ok) dp_fail(err, "out of memory capturing %s", program);
    }
    goto done;

spawn_failed:
    dp_fail(err, "cannot prepare to run %s", program);
done:
    if (pid > 0) {
        kill(-pid, SIGKILL);
        waitpid(pid, NULL, 0);
    }
    if (actions_ready) posix_spawn_file_actions_destroy(&actions);
    if (attr_ready) posix_spawnattr_destroy(&attr);
    for (int i = 0; i < 2; i++) {
        if (out_pipe[i] >= 0) close(out_pipe[i]);
        if (err_pipe[i] >= 0) close(err_pipe[i]);
    }
    free(argv);
    free(envp);
    return ok;
}

/* ------------------------------------------------------------- console */

bool dp_prompt_secret(const char *prompt, omt_buf *out, dp_err *err) {
    int fd = open("/dev/tty", O_RDWR | O_CLOEXEC | O_NOCTTY);
    if (fd < 0) {
        dp_fail_os(err, "cannot open the terminal for a password prompt");
        return false;
    }
    struct termios saved, quiet;
    bool restore = tcgetattr(fd, &saved) == 0;
    if (restore) {
        quiet = saved;
        quiet.c_lflag &= ~(tcflag_t)(ECHO | ECHONL);
        quiet.c_lflag |= ICANON;
        tcsetattr(fd, TCSAFLUSH, &quiet);
    }
    size_t prompt_len = strlen(prompt);
    if (write(fd, prompt, prompt_len) < 0) {
        /* The prompt is a courtesy; reading still works without it. */
    }
    bool ok = true;
    for (;;) {
        char c;
        ssize_t n = read(fd, &c, 1);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0 || c == '\n') break;
        if (out->len >= 4u * 1024u * 1024u) {
            ok = false;
            break;
        }
        omt_buf_putc(out, (uint8_t)c);
    }
    if (out->len > 0 && out->data[out->len - 1] == '\r') {
        out->data[--out->len] = 0;
    }
    if (restore) tcsetattr(fd, TCSAFLUSH, &saved);
    if (write(fd, "\n", 1) < 0) {
        /* As above. */
    }
    close(fd);
    if (!ok || out->failed) {
        dp_fail(err, "password input is too long");
        return false;
    }
    return true;
}

bool dp_read_stdin(size_t limit, omt_buf *out, bool *too_long, dp_err *err) {
    *too_long = false;
    uint8_t chunk[4096];
    for (;;) {
        ssize_t n = read(0, chunk, sizeof(chunk));
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) {
            dp_fail_os(err, "cannot read standard input");
            return false;
        }
        if (n == 0) return true;
        if (out->len + (size_t)n > limit) {
            *too_long = true;
            return true;
        }
        omt_buf_append(out, chunk, (size_t)n);
        if (out->failed) {
            dp_fail(err, "out of memory reading standard input");
            return false;
        }
    }
}
