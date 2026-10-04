/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include <signal.h>

#include "common/proc.h"
#include "test.h"

static void run(const char *const *argv, uint64_t timeout_ms, omt_proc_result *r) {
    omt_proc_options o = {argv, timeout_ms, 0, NULL, 0, NULL};
    omt_proc_run(&o, r);
}

static void captures_both_streams_and_exit_status(void) {
    const char *argv[] = {"/bin/sh", "-c", "printf out; printf err >&2; exit 7", NULL};
    omt_proc_result r;
    run(argv, 2000, &r);
    CHECK(r.has_returncode);
    CHECK_INT(r.returncode, 7);
    CHECK_STR(omt_buf_cstr(&r.out), "out");
    CHECK_STR(omt_buf_cstr(&r.err), "err");
    CHECK_STR(r.error, "");
    CHECK_STR(r.command, "/bin/sh -c printf out; printf err >&2; exit 7");
    omt_proc_result_free(&r);
}

static void timeout_includes_pipes_inherited_by_descendants(void) {
    const char *argv[] = {"/bin/sh", "-c", "sleep 3 & exit 0", NULL};
    omt_proc_result r;
    run(argv, 100, &r);
    CHECK(r.timed_out);
    CHECK(!r.has_returncode);
    CHECK(r.duration_seconds < 2.0);
    CHECK_STR(r.error, "Command exceeded 0.1 seconds.");
    omt_proc_result_free(&r);
}

static void timeout_reaps_a_running_child(void) {
    const char *argv[] = {"/bin/sh", "-c", "exec sleep 3", NULL};
    omt_proc_result r;
    run(argv, 100, &r);
    CHECK(r.timed_out);
    CHECK(r.duration_seconds < 2.0);
    omt_proc_result_free(&r);
}

static void verbose_commands_finish_after_both_capture_limits(void) {
    const char *argv[] = {"/bin/sh", "-c",
                          "head -c 300000 /dev/zero & head -c 300000 /dev/zero >&2 & wait", NULL};
    omt_proc_result r;
    run(argv, 5000, &r);
    CHECK(r.has_returncode && r.returncode == 0);
    CHECK(!r.timed_out);
    CHECK_INT(r.out.len, OMT_PROC_OUTPUT_LIMIT);
    CHECK_INT(r.err.len, OMT_PROC_OUTPUT_LIMIT);
    CHECK(r.stdout_truncated && r.stderr_truncated);
    omt_proc_result_free(&r);
}

static void missing_program_reports_spawn_failure(void) {
    const char *argv[] = {"/nonexistent/omt-command-test", NULL};
    omt_proc_result r;
    run(argv, 1000, &r);
    CHECK(!r.has_returncode);
    CHECK(r.error[0] != 0);
    CHECK(!r.timed_out);
    omt_proc_result_free(&r);
}

static void exact_limit_is_not_truncated_and_stdin_is_fed(void) {
    const char *argv[] = {"/bin/sh", "-c", "head -c 262144 /dev/zero", NULL};
    omt_proc_result r;
    run(argv, 5000, &r);
    CHECK_INT(r.out.len, OMT_PROC_OUTPUT_LIMIT);
    CHECK(!r.stdout_truncated);
    omt_proc_result_free(&r);

    const char *cat[] = {"/bin/cat", NULL};
    omt_proc_options o = {cat, 2000, 0, "fed\n", 4, NULL};
    omt_proc_run(&o, &r);
    CHECK_STR(omt_buf_cstr(&r.out), "fed\n");
    omt_proc_result_free(&r);
}

static void invalid_utf8_is_replaced(void) {
    const char *argv[] = {"/bin/sh", "-c", "printf 'a\\377b'", NULL};
    omt_proc_result r;
    run(argv, 2000, &r);
    CHECK_STR(omt_buf_cstr(&r.out), "a\xef\xbf\xbd"
                                    "b");
    omt_buf scratch;
    omt_buf_init(&scratch, 1024);
    CHECK_STR(omt_proc_report_text(&r, &scratch), "a\xef\xbf\xbd"
                                                  "b");
    omt_buf_free(&scratch);
    omt_proc_result_free(&r);
}

int main(void) {
    signal(SIGPIPE, SIG_IGN);
    RUN(captures_both_streams_and_exit_status);
    RUN(timeout_includes_pipes_inherited_by_descendants);
    RUN(timeout_reaps_a_running_child);
    RUN(verbose_commands_finish_after_both_capture_limits);
    RUN(missing_program_reports_spawn_failure);
    RUN(exact_limit_is_not_truncated_and_stdin_is_fed);
    RUN(invalid_utf8_is_replaced);
    return TEST_EXIT();
}
