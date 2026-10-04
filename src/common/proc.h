/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Bounded subprocesses.
 *
 * The program runs from an argv -- never through a shell -- with stdin on
 * /dev/null, in its own process group. Both output streams are captured up to
 * a per-stream limit and drained past it, so a verbose child can never block
 * on a full pipe. At the deadline the whole group is killed, which also stops
 * descendants that inherited the pipes. An exit code is reported only when the
 * command finished in time and both pipes reached end of file: output cut
 * short must not look like success.
 */
#ifndef OMT_PROC_H
#define OMT_PROC_H

#include "common/base.h"
#include "common/buf.h"

#define OMT_PROC_OUTPUT_LIMIT (256u * 1024u)

typedef struct {
    char *command; /* argv joined with spaces, for reports */
    bool has_returncode;
    int returncode;
    omt_buf out; /* lossily decoded UTF-8 */
    omt_buf err;
    double duration_seconds;
    bool timed_out;
    char error[256];
    bool stdout_truncated;
    bool stderr_truncated;
} omt_proc_result;

typedef struct {
    const char *const *argv; /* argv[0] is the program path; NULL-terminated */
    uint64_t timeout_ms;
    size_t output_limit;    /* 0 means OMT_PROC_OUTPUT_LIMIT */
    const void *stdin_data; /* NULL means /dev/null */
    size_t stdin_len;
    const char *const *envp; /* NULL inherits the environment */
} omt_proc_options;

void omt_proc_run(const omt_proc_options *options, omt_proc_result *result);
void omt_proc_result_free(omt_proc_result *result);
/* The most useful one-line explanation of a failure, as CommandResult did. */
const char *omt_proc_failure_detail(const omt_proc_result *result, omt_buf *scratch);
const char *omt_proc_report_text(const omt_proc_result *result, omt_buf *scratch);

/* Monotonic milliseconds, for deadlines. */
uint64_t omt_now_ms(void);
double omt_now_seconds(void);
/* Milliseconds left until a monotonic deadline, never negative. */
uint64_t omt_remaining_ms(uint64_t deadline_ms);

#endif
