/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The Windows half of the deployer's system layer. Paths and environment
 * values cross this boundary as UTF-8 and are converted to UTF-16 for the
 * wide Win32 APIs, so a path with non-ASCII characters works whatever the
 * system code page is.
 */
/* winsock2.h before windows.h, which would otherwise pull in the older
 * winsock.h. */
#include <winsock2.h>
#include <ws2tcpip.h>

#include <windows.h>

#include <process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

#include "common/json.h"
#include "deploy/core/sys.h"

/* --------------------------------------------------------------- UTF-16 */

static wchar_t *widen(const char *text) {
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text, -1, NULL, 0);
    if (n <= 0) return NULL;
    wchar_t *out = malloc((size_t)n * sizeof(wchar_t));
    if (out && MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text, -1, out, n) != n) {
        free(out);
        out = NULL;
    }
    return out;
}

static void narrow_into(const wchar_t *text, int len, omt_buf *out) {
    int n = WideCharToMultiByte(CP_UTF8, 0, text, len, NULL, 0, NULL, NULL);
    if (n <= 0) return;
    if (!omt_buf_reserve(out, (size_t)n + 1)) return;
    WideCharToMultiByte(CP_UTF8, 0, text, len, (char *)out->data + out->len, n, NULL, NULL);
    out->len += (size_t)n;
    out->data[out->len] = 0;
}

static void fail_code(dp_err *err, const char *what, DWORD code) {
    wchar_t *message = NULL;
    DWORD n = FormatMessageW(FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM |
                                 FORMAT_MESSAGE_IGNORE_INSERTS,
                             NULL, code, 0, (LPWSTR)&message, 0, NULL);
    omt_buf text;
    omt_buf_init(&text, 4096);
    if (n && message) {
        while (n > 0 &&
               (message[n - 1] == L'\r' || message[n - 1] == L'\n' || message[n - 1] == L'.'))
            n--;
        narrow_into(message, (int)n, &text);
    } else {
        omt_buf_puts(&text, "unknown error");
    }
    LocalFree(message);
    dp_fail(err, "%s: %s (os error %lu)", what, omt_buf_cstr(&text), (unsigned long)code);
    omt_buf_free(&text);
}

void dp_fail_os(dp_err *err, const char *what) { fail_code(err, what, GetLastError()); }

static void fail_socket(dp_err *err, const char *what) {
    fail_code(err, what, (DWORD)WSAGetLastError());
}

bool dp_sys_init(dp_err *err) {
    WSADATA data;
    int rc = WSAStartup(MAKEWORD(2, 2), &data);
    if (rc != 0) {
        fail_code(err, "cannot start Windows Sockets", (DWORD)rc);
        return false;
    }
    SetConsoleOutputCP(CP_UTF8);
    SetConsoleCP(CP_UTF8);
    return true;
}

uint64_t dp_now_ms(void) { return GetTickCount64(); }

void dp_sleep_ms(uint32_t ms) { Sleep(ms); }

/* -------------------------------------------------------------- sockets */

static int poll_one(SOCKET s, short events, uint32_t timeout_ms) {
    WSAPOLLFD p = {s, events, 0};
    return WSAPoll(&p, 1, timeout_ms > (uint32_t)INT32_MAX ? INT32_MAX : (INT)timeout_ms);
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
    struct addrinfo *list = NULL;
    int rc = getaddrinfo(host, service, &hints, &list);
    if (rc != 0) {
        fail_code(err, "cannot resolve host", (DWORD)rc);
        return false;
    }
    uint64_t deadline = dp_now_ms() + timeout_ms;
    int saved = WSAETIMEDOUT;
    for (struct addrinfo *ai = list; ai; ai = ai->ai_next) {
        uint64_t now = dp_now_ms();
        if (now >= deadline) break;
        SOCKET s = WSASocketW(ai->ai_family, ai->ai_socktype, ai->ai_protocol, NULL, 0,
                              WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT);
        if (s == INVALID_SOCKET) {
            saved = WSAGetLastError();
            continue;
        }
        u_long nonblocking = 1;
        ioctlsocket(s, (long)FIONBIO, &nonblocking);
        if (connect(s, ai->ai_addr, (int)ai->ai_addrlen) != 0) {
            if (WSAGetLastError() != WSAEWOULDBLOCK) {
                saved = WSAGetLastError();
                closesocket(s);
                continue;
            }
            int ready = poll_one(s, POLLWRNORM, (uint32_t)(deadline - now));
            int soerr = 0;
            int sl = sizeof(soerr);
            if (ready <= 0 || getsockopt(s, SOL_SOCKET, SO_ERROR, (char *)&soerr, &sl) != 0 ||
                soerr != 0) {
                saved = ready == 0 ? WSAETIMEDOUT : soerr ? soerr : WSAGetLastError();
                closesocket(s);
                continue;
            }
        }
        BOOL one = TRUE;
        setsockopt(s, IPPROTO_TCP, TCP_NODELAY, (const char *)&one, sizeof(one));
        freeaddrinfo(list);
        *out = (dp_sock)s;
        return true;
    }
    freeaddrinfo(list);
    if (saved == WSAETIMEDOUT) {
        dp_fail(err, "connecting to %s:%u timed out", host, (unsigned)port);
    } else {
        char what[300];
        omt_snprintf(what, sizeof(what), "cannot connect to %s:%u", host, (unsigned)port);
        fail_code(err, what, (DWORD)saved);
    }
    return false;
}

bool dp_sock_send(dp_sock s, const void *data, size_t len, uint32_t timeout_ms, dp_err *err) {
    const char *p = data;
    uint64_t deadline = dp_now_ms() + timeout_ms;
    while (len > 0) {
        int chunk = len > (1u << 30) ? (1 << 30) : (int)len;
        int n = send((SOCKET)s, p, chunk, 0);
        if (n > 0) {
            p += n;
            len -= (size_t)n;
            continue;
        }
        if (n < 0 && WSAGetLastError() == WSAEWOULDBLOCK) {
            uint64_t now = dp_now_ms();
            if (now >= deadline ||
                poll_one((SOCKET)s, POLLWRNORM, (uint32_t)(deadline - now)) == 0) {
                dp_fail(err, "network write timed out");
                return false;
            }
            continue;
        }
        fail_socket(err, "network write failed");
        return false;
    }
    return true;
}

long dp_sock_recv(dp_sock s, void *buf, size_t len, uint32_t timeout_ms, dp_err *err) {
    for (;;) {
        int n = recv((SOCKET)s, buf, len > (1u << 30) ? (1 << 30) : (int)len, 0);
        if (n >= 0) return n;
        if (WSAGetLastError() != WSAEWOULDBLOCK) {
            fail_socket(err, "network read failed");
            return -1;
        }
        int ready = poll_one((SOCKET)s, POLLRDNORM, timeout_ms);
        if (ready == 0) return -2;
        if (ready < 0) {
            fail_socket(err, "network wait failed");
            return -1;
        }
    }
}

int dp_sock_wait(dp_sock s, bool write, uint32_t timeout_ms) {
    int rc = poll_one((SOCKET)s, write ? POLLWRNORM : POLLRDNORM, timeout_ms);
    return rc > 0 ? 1 : rc < 0 ? -1 : 0;
}

void dp_sock_close(dp_sock s) {
    if (s != DP_SOCK_INVALID) closesocket((SOCKET)s);
}

/* --------------------------------------------------------------- threads */

typedef struct {
    dp_thread_fn fn;
    void *arg;
} thread_start;

static unsigned __stdcall thread_main(void *raw) {
    thread_start start = *(thread_start *)raw;
    free(raw);
    start.fn(start.arg);
    return 0;
}

bool dp_thread_spawn(dp_thread_fn fn, void *arg, dp_err *err) {
    thread_start *start = malloc(sizeof(*start));
    if (!start) {
        dp_fail(err, "out of memory");
        return false;
    }
    start->fn = fn;
    start->arg = arg;
    uintptr_t handle = _beginthreadex(NULL, 2u * 1024u * 1024u, thread_main, start, 0, NULL);
    if (handle == 0) {
        free(start);
        dp_fail_os(err, "cannot start a worker thread");
        return false;
    }
    CloseHandle((HANDLE)handle);
    return true;
}

struct dp_mutex {
    SRWLOCK lock;
};

dp_mutex *dp_mutex_new(void) {
    dp_mutex *m = malloc(sizeof(*m));
    if (m) InitializeSRWLock(&m->lock);
    return m;
}
void dp_mutex_free(dp_mutex *m) { free(m); }
void dp_mutex_lock(dp_mutex *m) { AcquireSRWLockExclusive(&m->lock); }
void dp_mutex_unlock(dp_mutex *m) { ReleaseSRWLockExclusive(&m->lock); }

/* ----------------------------------------------------------------- files */

static bool attributes(const char *path, WIN32_FILE_ATTRIBUTE_DATA *data, DWORD *error) {
    wchar_t *wide = widen(path);
    if (!wide) {
        *error = ERROR_INVALID_NAME;
        return false;
    }
    BOOL ok = GetFileAttributesExW(wide, GetFileExInfoStandard, data);
    *error = ok ? 0 : GetLastError();
    free(wide);
    return ok != 0;
}

static dp_path_kind missing_or_error(DWORD error) {
    return error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND ? DP_PATH_MISSING
                                                                          : DP_PATH_ERROR;
}

dp_path_kind dp_path_lstat(const char *path) {
    WIN32_FILE_ATTRIBUTE_DATA data;
    DWORD error;
    if (!attributes(path, &data, &error)) return missing_or_error(error);
    if (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) return DP_PATH_LINK;
    if (data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) return DP_PATH_DIR;
    return DP_PATH_FILE;
}

static HANDLE open_followed(const char *path, DWORD access, DWORD *error) {
    wchar_t *wide = widen(path);
    if (!wide) {
        *error = ERROR_INVALID_NAME;
        return INVALID_HANDLE_VALUE;
    }
    HANDLE h = CreateFileW(wide, access, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           NULL, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, NULL);
    *error = h == INVALID_HANDLE_VALUE ? GetLastError() : 0;
    free(wide);
    return h;
}

dp_path_kind dp_path_stat(const char *path) {
    DWORD error;
    HANDLE h = open_followed(path, 0, &error);
    if (h == INVALID_HANDLE_VALUE) return missing_or_error(error);
    BY_HANDLE_FILE_INFORMATION info;
    BOOL ok = GetFileInformationByHandle(h, &info);
    CloseHandle(h);
    if (!ok) return DP_PATH_ERROR;
    return info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY ? DP_PATH_DIR : DP_PATH_FILE;
}

bool dp_is_file(const char *path) { return dp_path_stat(path) == DP_PATH_FILE; }
bool dp_is_dir(const char *path) { return dp_path_stat(path) == DP_PATH_DIR; }
/* Windows has no execute bit: a file is runnable by name, as Rust treated it. */
bool dp_is_executable_file(const char *path) { return dp_is_file(path); }

bool dp_file_size(const char *path, uint64_t *size) {
    DWORD error;
    HANDLE h = open_followed(path, 0, &error);
    if (h == INVALID_HANDLE_VALUE) return false;
    BY_HANDLE_FILE_INFORMATION info;
    BOOL ok =
        GetFileInformationByHandle(h, &info) && !(info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY);
    CloseHandle(h);
    if (ok) *size = ((uint64_t)info.nFileSizeHigh << 32) | info.nFileSizeLow;
    return ok != 0;
}

/* Size and modification time, as the Rust deployer's non-Unix fingerprint
 * was: Windows has no device and inode pair to add. */
bool dp_file_fingerprint(const char *path, omt_buf *out, dp_err *err) {
    WIN32_FILE_ATTRIBUTE_DATA data;
    DWORD error;
    if (!attributes(path, &data, &error) ||
        (data.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT))) {
        dp_fail(err, "deployment artifact is missing or unsafe: %s", path);
        return false;
    }
    uint64_t size = ((uint64_t)data.nFileSizeHigh << 32) | data.nFileSizeLow;
    uint64_t ticks =
        ((uint64_t)data.ftLastWriteTime.dwHighDateTime << 32) | data.ftLastWriteTime.dwLowDateTime;
    omt_buf_printf(out, "%llu:%llu", (unsigned long long)size, (unsigned long long)ticks);
    return !out->failed;
}

struct dp_file {
    HANDLE h;
};

dp_file *dp_file_open_read(const char *path, dp_err *err) {
    DWORD error;
    HANDLE h = open_followed(path, GENERIC_READ, &error);
    if (h == INVALID_HANDLE_VALUE) {
        fail_code(err, path, error);
        return NULL;
    }
    dp_file *f = malloc(sizeof(*f));
    if (!f) {
        CloseHandle(h);
        dp_fail(err, "out of memory");
        return NULL;
    }
    f->h = h;
    return f;
}

long dp_file_read(dp_file *f, void *buf, size_t len, dp_err *err) {
    DWORD got = 0;
    DWORD want = len > (1u << 30) ? (1u << 30) : (DWORD)len;
    if (!ReadFile(f->h, buf, want, &got, NULL)) {
        dp_fail_os(err, "read failed");
        return -1;
    }
    return (long)got;
}

bool dp_file_rewind(dp_file *f, dp_err *err) {
    LARGE_INTEGER zero = {0};
    if (SetFilePointerEx(f->h, zero, NULL, FILE_BEGIN)) return true;
    dp_fail_os(err, "seek failed");
    return false;
}

void dp_file_close(dp_file *f) {
    if (!f) return;
    CloseHandle(f->h);
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
    wchar_t *wide = widen(path);
    if (!wide) {
        dp_fail(err, "invalid path: %s", path);
        return false;
    }
    HANDLE h = CreateFileW(wide, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS,
                           FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, NULL);
    free(wide);
    if (h == INVALID_HANDLE_VALUE) {
        dp_fail_os(err, path);
        return false;
    }
    const uint8_t *p = data;
    while (len > 0) {
        DWORD wrote = 0;
        DWORD want = len > (1u << 30) ? (1u << 30) : (DWORD)len;
        if (!WriteFile(h, p, want, &wrote, NULL) || wrote == 0) {
            dp_fail_os(err, path);
            CloseHandle(h);
            return false;
        }
        p += wrote;
        len -= wrote;
    }
    bool ok = FlushFileBuffers(h) != 0;
    if (!ok) dp_fail_os(err, path);
    CloseHandle(h);
    return ok;
}

/* ------------------------------------------------------------ environment */

bool dp_getenv(const char *name, omt_buf *out) {
    wchar_t *wide = widen(name);
    if (!wide) return false;
    DWORD n = GetEnvironmentVariableW(wide, NULL, 0);
    bool found = false;
    if (n > 0) {
        wchar_t *value = malloc((size_t)n * sizeof(wchar_t));
        if (value) {
            DWORD got = GetEnvironmentVariableW(wide, value, n);
            if (got > 0 && got < n) {
                narrow_into(value, (int)got, out);
                found = true;
            } else if (got == 0 && GetLastError() == 0) {
                found = true;
            }
            free(value);
        }
    }
    free(wide);
    return found;
}

void dp_current_dir(omt_buf *out) {
    wchar_t path[32768];
    DWORD n = GetCurrentDirectoryW(32768, path);
    if (n > 0 && n < 32768) {
        narrow_into(path, (int)n, out);
    } else {
        omt_buf_puts(out, ".");
    }
}

/* ------------------------------------------------------------ processes */

void dp_process_result_free(dp_process_result *result) { omt_buf_free(&result->output); }

/* One argument quoted by the rules CommandLineToArgvW and the C runtime
 * use to split a command line back into an argv. */
static void quote_arg(omt_buf *out, const char *arg) {
    bool needs = arg[0] == 0 || strpbrk(arg, " \t\n\v\"") != NULL;
    if (!needs) {
        omt_buf_puts(out, arg);
        return;
    }
    omt_buf_putc(out, '"');
    for (const char *p = arg;; p++) {
        size_t backslashes = 0;
        while (*p == '\\') {
            backslashes++;
            p++;
        }
        if (!*p) {
            for (size_t i = 0; i < backslashes * 2; i++) omt_buf_putc(out, '\\');
            break;
        }
        if (*p == '"') {
            for (size_t i = 0; i < backslashes * 2 + 1; i++) omt_buf_putc(out, '\\');
        } else {
            for (size_t i = 0; i < backslashes; i++) omt_buf_putc(out, '\\');
        }
        omt_buf_putc(out, (uint8_t)*p);
    }
    omt_buf_putc(out, '"');
}

/* The inherited environment with the additions, as a UTF-16 block. */
static wchar_t *environment_block(const char *const *env) {
    wchar_t *base = GetEnvironmentStringsW();
    if (!base) return NULL;
    size_t extra = 0;
    while (env && env[extra]) extra++;
    wchar_t **added = calloc(extra + 1, sizeof(wchar_t *));
    size_t total = 1;
    for (size_t i = 0; added && i < extra; i++) {
        added[i] = widen(env[i]);
        if (added[i]) total += wcslen(added[i]) + 1;
    }
    for (const wchar_t *p = base; *p; p += wcslen(p) + 1) total += wcslen(p) + 1;
    wchar_t *block = added ? malloc(total * sizeof(wchar_t)) : NULL;
    if (block) {
        wchar_t *w = block;
        for (const wchar_t *p = base; *p; p += wcslen(p) + 1) {
            const wchar_t *eq = wcschr(p + 1, L'=');
            size_t name_len = eq ? (size_t)(eq - p) : wcslen(p);
            bool replaced = false;
            for (size_t i = 0; i < extra; i++) {
                if (added[i] && _wcsnicmp(added[i], p, name_len) == 0 &&
                    added[i][name_len] == L'=') {
                    replaced = true;
                }
            }
            if (replaced) continue;
            size_t n = wcslen(p) + 1;
            memcpy(w, p, n * sizeof(wchar_t));
            w += n;
        }
        for (size_t i = 0; i < extra; i++) {
            if (!added[i]) continue;
            size_t n = wcslen(added[i]) + 1;
            memcpy(w, added[i], n * sizeof(wchar_t));
            w += n;
        }
        *w = 0;
    }
    for (size_t i = 0; added && i < extra; i++) free(added[i]);
    free(added);
    FreeEnvironmentStringsW(base);
    return block;
}

static void pump(HANDLE pipe, omt_buf *into, bool *open) {
    for (;;) {
        DWORD available = 0;
        if (!PeekNamedPipe(pipe, NULL, 0, NULL, &available, NULL)) {
            *open = false; /* broken pipe: the writer has gone */
            return;
        }
        if (available == 0) return;
        uint8_t chunk[8192];
        DWORD got = 0;
        DWORD want = available < sizeof(chunk) ? available : (DWORD)sizeof(chunk);
        if (!ReadFile(pipe, chunk, want, &got, NULL) || got == 0) {
            *open = false;
            return;
        }
        size_t room = DP_OUTPUT_LIMIT > into->len ? DP_OUTPUT_LIMIT - into->len : 0;
        omt_buf_append(into, chunk, omt_min_size(got, room));
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
    omt_buf line;
    omt_buf_init(&line, 64u * 1024u);
    quote_arg(&line, program);
    for (size_t i = 0; args && args[i]; i++) {
        omt_buf_putc(&line, ' ');
        quote_arg(&line, args[i]);
    }
    wchar_t *wide_program = widen(program);
    wchar_t *wide_line = widen(omt_buf_cstr(&line));
    wchar_t *wide_dir = directory ? widen(directory) : NULL;
    wchar_t *block = environment_block(env);
    omt_buf_free(&line);

    SECURITY_ATTRIBUTES inherit = {sizeof(inherit), NULL, TRUE};
    HANDLE out_r = NULL, out_w = NULL, err_r = NULL, err_w = NULL, nul = INVALID_HANDLE_VALUE;
    HANDLE job = NULL;
    PROCESS_INFORMATION pi = {0};
    bool ok = false, started = false;
    if (!wide_program || !wide_line || !block || (directory && !wide_dir)) {
        dp_fail(err, "cannot run %s: invalid text in the command", program);
        goto done;
    }
    if (!CreatePipe(&out_r, &out_w, &inherit, 0) || !CreatePipe(&err_r, &err_w, &inherit, 0)) {
        dp_fail_os(err, "cannot create a pipe");
        goto done;
    }
    SetHandleInformation(out_r, HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(err_r, HANDLE_FLAG_INHERIT, 0);
    nul = CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, &inherit,
                      OPEN_EXISTING, 0, NULL);
    /* A job object, so cancelling stops the whole process tree. */
    job = CreateJobObjectW(NULL, NULL);
    if (job) {
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits;
        memset(&limits, 0, sizeof(limits));
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits, sizeof(limits));
    }
    STARTUPINFOW si;
    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = nul;
    si.hStdOutput = out_w;
    si.hStdError = err_w;
    if (!CreateProcessW(wide_program, wide_line, NULL, NULL, TRUE,
                        CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | CREATE_SUSPENDED, block,
                        wide_dir, &si, &pi)) {
        char what[4200];
        omt_snprintf(what, sizeof(what), "cannot run %s", program);
        dp_fail_os(err, what);
        goto done;
    }
    started = true;
    if (job) AssignProcessToJobObject(job, pi.hProcess);
    ResumeThread(pi.hThread);
    CloseHandle(out_w);
    CloseHandle(err_w);
    out_w = err_w = NULL;

    omt_buf out_bytes, err_bytes;
    omt_buf_init(&out_bytes, DP_OUTPUT_LIMIT + 1);
    omt_buf_init(&err_bytes, DP_OUTPUT_LIMIT + 1);
    bool out_open = true, err_open = true, exited = false;
    for (;;) {
        if (dp_cancelled(cancel)) {
            if (job) TerminateJobObject(job, 1);
            TerminateProcess(pi.hProcess, 1);
            omt_buf_free(&out_bytes);
            omt_buf_free(&err_bytes);
            dp_fail_cancelled(err);
            goto done;
        }
        if (!exited) exited = WaitForSingleObject(pi.hProcess, 25) == WAIT_OBJECT_0;
        if (out_open) pump(out_r, &out_bytes, &out_open);
        if (err_open) pump(err_r, &err_bytes, &err_open);
        if (exited && !out_open && !err_open) break;
        if (exited) Sleep(5);
    }
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    result->exit_code = (int)code;
    size_t room = DP_OUTPUT_LIMIT > out_bytes.len ? DP_OUTPUT_LIMIT - out_bytes.len : 0;
    omt_buf_append(&out_bytes, err_bytes.data, omt_min_size(err_bytes.len, room));
    omt_utf8_lossy(&result->output, (const char *)out_bytes.data, out_bytes.len);
    omt_buf_free(&out_bytes);
    omt_buf_free(&err_bytes);
    ok = !result->output.failed;
    if (!ok) dp_fail(err, "out of memory capturing %s", program);
done:
    if (started) {
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
    if (job) CloseHandle(job);
    if (out_r) CloseHandle(out_r);
    if (out_w) CloseHandle(out_w);
    if (err_r) CloseHandle(err_r);
    if (err_w) CloseHandle(err_w);
    if (nul != INVALID_HANDLE_VALUE) CloseHandle(nul);
    free(wide_program);
    free(wide_line);
    free(wide_dir);
    free(block);
    return ok;
}

/* ------------------------------------------------------------- console */

bool dp_prompt_secret(const char *prompt, omt_buf *out, dp_err *err) {
    HANDLE in = CreateFileW(L"CONIN$", GENERIC_READ | GENERIC_WRITE,
                            FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, 0, NULL);
    HANDLE con = CreateFileW(L"CONOUT$", GENERIC_READ | GENERIC_WRITE,
                             FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, 0, NULL);
    if (in == INVALID_HANDLE_VALUE || con == INVALID_HANDLE_VALUE) {
        if (in != INVALID_HANDLE_VALUE) CloseHandle(in);
        if (con != INVALID_HANDLE_VALUE) CloseHandle(con);
        dp_fail_os(err, "cannot open the console for a password prompt");
        return false;
    }
    DWORD mode = 0;
    bool restore = GetConsoleMode(in, &mode) != 0;
    if (restore)
        SetConsoleMode(in, (mode | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT) &
                               ~(DWORD)ENABLE_ECHO_INPUT);
    wchar_t *wide_prompt = widen(prompt);
    if (wide_prompt) {
        DWORD wrote;
        WriteConsoleW(con, wide_prompt, (DWORD)wcslen(wide_prompt), &wrote, NULL);
        free(wide_prompt);
    }
    wchar_t buffer[4096];
    DWORD got = 0;
    bool ok = ReadConsoleW(in, buffer, 4096, &got, NULL) != 0;
    while (got > 0 && (buffer[got - 1] == L'\n' || buffer[got - 1] == L'\r')) got--;
    if (ok) narrow_into(buffer, (int)got, out);
    SecureZeroMemory(buffer, sizeof(buffer));
    if (restore) SetConsoleMode(in, mode);
    DWORD wrote;
    WriteConsoleW(con, L"\r\n", 2, &wrote, NULL);
    CloseHandle(in);
    CloseHandle(con);
    if (!ok || out->failed) {
        dp_fail(err, "cannot read the password");
        return false;
    }
    return true;
}

bool dp_read_stdin(size_t limit, omt_buf *out, bool *too_long, dp_err *err) {
    *too_long = false;
    HANDLE in = GetStdHandle(STD_INPUT_HANDLE);
    uint8_t chunk[4096];
    for (;;) {
        DWORD got = 0;
        if (!ReadFile(in, chunk, sizeof(chunk), &got, NULL)) {
            if (GetLastError() == ERROR_BROKEN_PIPE) return true;
            dp_fail_os(err, "cannot read standard input");
            return false;
        }
        if (got == 0) return true;
        if (out->len + got > limit) {
            *too_long = true;
            return true;
        }
        omt_buf_append(out, chunk, got);
        if (out->failed) {
            dp_fail(err, "out of memory reading standard input");
            return false;
        }
    }
}
