/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The workstation rules behind dp_probe_prerequisites, with the platform
 * passed in so the Windows answers are tested on Linux.
 */
#ifndef DP_TOOLS_INTERNAL_H
#define DP_TOOLS_INTERNAL_H

#include "common/buf.h"
#include "deploy/core/deploy.h"

#define DP_MAX_SUFFIXES 32
#define DP_MAX_CANDIDATES 16

extern const int DP_WINGET_ALREADY_INSTALLED;

/* The file-name suffixes an executable may carry: the empty suffix first,
 * then PATHEXT's entries on Windows, or .COM .EXE .BAT .CMD when PATHEXT is
 * unset or unusable. */
typedef struct {
    char *items[DP_MAX_SUFFIXES];
    size_t count;
} dp_suffixes;

void dp_executable_suffixes(bool windows, const char *pathext, dp_suffixes *out);
void dp_suffixes_free(dp_suffixes *s);

/* Whether a bash.exe is Windows' WSL launcher rather than a real shell. */
bool dp_is_wsl_launcher(const char *path);

typedef bool (*dp_env_lookup)(void *ctx, const char *name, omt_buf *out);

/* Where Git for Windows and MSYS2 put bash, in preference order. */
typedef struct {
    char *items[DP_MAX_CANDIDATES];
    size_t count;
} dp_candidates;

void dp_windows_bash_candidates(dp_env_lookup lookup, void *ctx, dp_candidates *out);
void dp_candidates_free(dp_candidates *c);

bool dp_find_bash(omt_buf *out);
bool dp_find_container_engine(omt_buf *out, const char **kind);
/* The image build command for make and bash as probed (either may be NULL). */
bool dp_plan_from(const char *make, const char *bash, bool windows, dp_build_plan *plan,
                  dp_err *err);
bool dp_report_install(const dp_package *package, const dp_process_result *result,
                       const dp_progress *p, dp_err *err);
bool dp_reports_aarch64(const dp_process_result *result);
void dp_emulation_failure(const char *kind, bool windows, bool repaired, const char *output,
                          omt_buf *out);

#endif
