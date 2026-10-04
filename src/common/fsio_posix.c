/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/rand.h"

static bool write_all(int fd, const uint8_t *data, size_t len, omt_err *err) {
    while (len) {
        ssize_t n = write(fd, data, len);
        if (n < 0) {
            if (errno == EINTR) continue;
            omt_err_os(err, errno);
            return false;
        }
        data += n;
        len -= (size_t)n;
    }
    return true;
}

omt_read_result omt_read_bounded(const char *path, size_t maximum, omt_buf *out, omt_err *err) {
    omt_buf_init(out, maximum);
    struct stat before;
    if (lstat(path, &before) != 0) {
        if (errno == ENOENT) return OMT_READ_MISSING;
        omt_err_os(err, errno);
        return OMT_READ_ERROR;
    }
    if (!S_ISREG(before.st_mode)) {
        omt_err_set(err, "path is not a safe regular file");
        return OMT_READ_ERROR;
    }
    if ((uint64_t)before.st_size > (uint64_t)maximum) {
        omt_err_set(err, "file exceeds its size limit");
        return OMT_READ_ERROR;
    }
    int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) {
        omt_err_os(err, errno);
        return OMT_READ_ERROR;
    }
    struct stat opened;
    if (fstat(fd, &opened) != 0 || opened.st_dev != before.st_dev ||
        opened.st_ino != before.st_ino || !S_ISREG(opened.st_mode)) {
        close(fd);
        omt_err_set(err, "file changed while opening");
        return OMT_READ_ERROR;
    }
    uint8_t chunk[8192];
    for (;;) {
        ssize_t n = read(fd, chunk, sizeof(chunk));
        if (n < 0) {
            if (errno == EINTR) continue;
            omt_err_os(err, errno);
            close(fd);
            omt_buf_free(out);
            return OMT_READ_ERROR;
        }
        if (n == 0) break;
        if (out->len + (size_t)n > maximum) {
            close(fd);
            omt_buf_free(out);
            omt_err_set(err, "file exceeds its size limit");
            return OMT_READ_ERROR;
        }
        omt_buf_append(out, chunk, (size_t)n);
    }
    close(fd);
    if (out->failed) {
        omt_buf_free(out);
        omt_err_set(err, "file exceeds its size limit");
        return OMT_READ_ERROR;
    }
    /* An empty read still yields a usable, NUL-terminated buffer. */
    omt_buf_append(out, "", 0);
    return OMT_READ_OK;
}

omt_read_result omt_read_text(const char *path, size_t maximum, omt_buf *out, omt_err *err) {
    omt_read_result r = omt_read_bounded(path, maximum, out, err);
    if (r == OMT_READ_OK && !omt_utf8_valid(omt_buf_cstr(out), out->len)) {
        omt_buf_free(out);
        omt_err_set(err, "file is not valid UTF-8");
        return OMT_READ_ERROR;
    }
    return r;
}

/* Splits path into its directory (or ".") and its final component. */
static bool split_path(const char *path, char *dir, size_t dir_size, const char **name) {
    const char *slash = strrchr(path, '/');
    if (!slash) {
        *name = path;
        return omt_strlcpy(dir, ".", dir_size);
    }
    *name = slash + 1;
    size_t n = (size_t)(slash - path);
    if (n == 0) return omt_strlcpy(dir, "/", dir_size);
    if (n >= dir_size) return false;
    memcpy(dir, path, n);
    dir[n] = 0;
    return true;
}

static bool sync_directory(const char *dir, omt_err *err) {
    int fd = open(dir, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (fd < 0) {
        omt_err_os(err, errno);
        return false;
    }
    bool ok = fsync(fd) == 0;
    if (!ok) omt_err_os(err, errno);
    close(fd);
    return ok;
}

bool omt_atomic_replace(const char *path, const void *data, size_t len, size_t maximum,
                        omt_err *err) {
    if (len > maximum) {
        omt_err_set(err, "replacement exceeds its size limit");
        return false;
    }
    char dir[4096];
    const char *name;
    if (!split_path(path, dir, sizeof(dir), &name) || !*name) {
        omt_err_set(err, "invalid target path");
        return false;
    }
    struct stat st;
    if (lstat(dir, &st) != 0) {
        omt_err_os(err, errno);
        return false;
    }
    if (!S_ISDIR(st.st_mode)) {
        omt_err_set(err, "target directory is unsafe");
        return false;
    }
    if (lstat(path, &st) == 0 && !S_ISREG(st.st_mode)) {
        omt_err_set(err, "target path is unsafe");
        return false;
    }
    omt_buf nonce;
    omt_buf_init(&nonce, 64);
    if (!omt_random_hex(&nonce, 12, err)) {
        omt_buf_free(&nonce);
        return false;
    }
    char staged[4096 + 300];
    bool fits = strcmp(dir, ".") == 0 && !strchr(path, '/')
                    ? omt_snprintf(staged, sizeof(staged), ".%s.%s.tmp", name, omt_buf_cstr(&nonce))
                    : omt_snprintf(staged, sizeof(staged), "%s/.%s.%s.tmp",
                                   strcmp(dir, "/") == 0 ? "" : dir, name, omt_buf_cstr(&nonce));
    omt_buf_free(&nonce);
    if (!fits) {
        omt_err_set(err, "invalid target path");
        return false;
    }
    int fd = open(staged, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0) {
        omt_err_os(err, errno);
        return false;
    }
    bool ok = write_all(fd, data, len, err);
    if (ok && fsync(fd) != 0) {
        omt_err_os(err, errno);
        ok = false;
    }
    close(fd);
    if (ok && rename(staged, path) != 0) {
        omt_err_os(err, errno);
        ok = false;
    }
    if (ok) ok = sync_directory(dir, err);
    if (!ok) unlink(staged);
    return ok;
}

bool omt_remove_file_durable(const char *path, omt_err *err) {
    struct stat st;
    if (lstat(path, &st) != 0) {
        if (errno == ENOENT) return true;
        omt_err_os(err, errno);
        return false;
    }
    if (!S_ISREG(st.st_mode)) {
        omt_err_set(err, "target path is unsafe");
        return false;
    }
    if (unlink(path) != 0) {
        omt_err_os(err, errno);
        return false;
    }
    char dir[4096];
    const char *name;
    if (!split_path(path, dir, sizeof(dir), &name)) {
        omt_err_set(err, "target has no parent");
        return false;
    }
    return sync_directory(dir, err);
}

bool omt_write_fixed_inode(const char *path, const void *data, size_t len, size_t maximum,
                           omt_err *err) {
    if (len > maximum) {
        omt_err_set(err, "request exceeds its size limit");
        return false;
    }
    struct stat before, opened, after;
    if (lstat(path, &before) != 0) {
        omt_err_os(err, errno);
        return false;
    }
    if (!S_ISREG(before.st_mode)) {
        omt_err_set(err, "request file is unsafe");
        return false;
    }
    int fd = open(path, O_WRONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) {
        omt_err_os(err, errno);
        return false;
    }
    if (fstat(fd, &opened) != 0 || opened.st_dev != before.st_dev ||
        opened.st_ino != before.st_ino || !S_ISREG(opened.st_mode)) {
        close(fd);
        omt_err_set(err, "request file changed while opening");
        return false;
    }
    bool ok = true;
    if (ftruncate(fd, 0) != 0 || lseek(fd, 0, SEEK_SET) != 0) {
        omt_err_os(err, errno);
        ok = false;
    }
    if (ok) ok = write_all(fd, data, len, err);
    if (ok && fsync(fd) != 0) {
        omt_err_os(err, errno);
        ok = false;
    }
    close(fd);
    if (!ok) return false;
    if (lstat(path, &after) != 0) {
        omt_err_os(err, errno);
        return false;
    }
    if (S_ISLNK(after.st_mode) || after.st_dev != before.st_dev || after.st_ino != before.st_ino) {
        omt_err_set(err, "request file changed during write");
        return false;
    }
    return true;
}

bool omt_mkdir_all(const char *path, unsigned mode, omt_err *err) {
    char buf[4096];
    if (!omt_strlcpy(buf, path, sizeof(buf))) {
        omt_err_set(err, "path too long");
        return false;
    }
    size_t len = strlen(buf);
    for (size_t i = 1; i <= len; i++) {
        if (buf[i] != '/' && buf[i] != 0) continue;
        char saved = buf[i];
        buf[i] = 0;
        if (mkdir(buf, (mode_t)mode) != 0 && errno != EEXIST) {
            omt_err_os(err, errno);
            return false;
        }
        buf[i] = saved;
    }
    struct stat st;
    if (stat(path, &st) != 0 || !S_ISDIR(st.st_mode)) {
        omt_err_set(err, "File exists (os error 17)");
        return false;
    }
    return true;
}
