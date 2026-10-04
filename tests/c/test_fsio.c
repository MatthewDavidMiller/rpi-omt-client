/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

#include "common/fsio.h"
#include "test.h"

static char dir[256];

static void path(char *out, size_t n, const char *name) { snprintf(out, n, "%s/%s", dir, name); }

static void reads_are_bounded_and_refuse_links(void) {
    char p[512], link[512];
    path(p, sizeof(p), "a.txt");
    path(link, sizeof(link), "link.txt");
    omt_err err;
    CHECK(omt_atomic_replace(p, "hello", 5, 16, &err));
    omt_buf b;
    CHECK_INT(omt_read_bounded(p, 16, &b, &err), OMT_READ_OK);
    CHECK_STR(omt_buf_cstr(&b), "hello");
    omt_buf_free(&b);
    CHECK_INT(omt_read_bounded(p, 4, &b, &err), OMT_READ_ERROR);
    CHECK_STR(err.msg, "file exceeds its size limit");
    CHECK(symlink(p, link) == 0);
    CHECK_INT(omt_read_bounded(link, 16, &b, &err), OMT_READ_ERROR);
    CHECK_STR(err.msg, "path is not a safe regular file");
    char missing[512];
    path(missing, sizeof(missing), "missing");
    CHECK_INT(omt_read_bounded(missing, 16, &b, &err), OMT_READ_MISSING);
    CHECK(!omt_atomic_replace(link, "x", 1, 16, &err));
    CHECK_STR(err.msg, "target path is unsafe");
    CHECK(!omt_atomic_replace(p, "toolong", 7, 4, &err));
    struct stat st;
    CHECK(stat(p, &st) == 0 && (st.st_mode & 0777) == 0600);
    CHECK(omt_remove_file_durable(p, &err));
    CHECK(omt_remove_file_durable(p, &err));
    CHECK(!omt_remove_file_durable(link, &err));
    unlink(link);
}

static void text_reads_require_utf8(void) {
    char p[512];
    path(p, sizeof(p), "bad.txt");
    omt_err err;
    CHECK(omt_atomic_replace(p, "\xff", 1, 16, &err));
    omt_buf b;
    CHECK_INT(omt_read_text(p, 16, &b, &err), OMT_READ_ERROR);
    CHECK_STR(err.msg, "file is not valid UTF-8");
    unlink(p);
}

static void fixed_inode_writes_keep_the_inode(void) {
    char p[512];
    path(p, sizeof(p), "request");
    omt_err err;
    CHECK(!omt_write_fixed_inode(p, "x", 1, 16, &err));
    FILE *f = fopen(p, "w");
    CHECK(f != NULL);
    if (!f) return;
    fputs("previous contents", f);
    fclose(f);
    struct stat before, after;
    stat(p, &before);
    CHECK(omt_write_fixed_inode(p, "new", 3, 16, &err));
    stat(p, &after);
    CHECK(before.st_ino == after.st_ino);
    omt_buf b;
    CHECK_INT(omt_read_bounded(p, 64, &b, &err), OMT_READ_OK);
    CHECK_STR(omt_buf_cstr(&b), "new");
    omt_buf_free(&b);
    unlink(p);
}

int main(void) {
    const char *tmp = getenv("TMPDIR");
    snprintf(dir, sizeof(dir), "%s/omt-fsio-XXXXXX", tmp ? tmp : "/tmp");
    if (!mkdtemp(dir)) return 1;
    RUN(reads_are_bounded_and_refuse_links);
    RUN(text_reads_require_utf8);
    RUN(fixed_inode_writes_keep_the_inode);
    rmdir(dir);
    return TEST_EXIT();
}
