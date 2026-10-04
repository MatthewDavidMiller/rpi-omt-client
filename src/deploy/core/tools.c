/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * What a workstation must provide before it can rebuild the appliance.
 *
 * An operator's deployment needs none of this: the capsule is compiled into
 * the binary, so nothing here is probed unless a project root is named. What
 * remains is the developer path, where the image is built on the machine the
 * deployer runs on -- sharpest on Windows, where a stock install has neither a
 * POSIX shell nor a container engine.
 *
 * Every rule is a function over probed values with the platform passed in, so
 * the Windows answers are tested on a Linux workstation, the only machine the
 * gates run on. The probes themselves are the thin part.
 */
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "deploy/core/deploy.h"
#include "deploy/core/tools_internal.h"

/* The same pinned probe image scripts/check-arm64-emulation.sh uses. */
#define EMULATION_PROBE_IMAGE                                                                      \
    "docker.io/library/debian:bookworm-slim@sha256:"                                               \
    "4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d"
/* The same pinned binfmt installer scripts/install-arm64-emulation.sh takes
 * its emulator from. Run with --install, it registers the handler in the
 * kernel it runs against. */
#define BINFMT_INSTALLER_IMAGE                                                                     \
    "docker.io/tonistiigi/binfmt@sha256:"                                                          \
    "400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0"
#define BINFMT_ARCHITECTURES "arm64,arm"

const dp_package DP_GIT_FOR_WINDOWS = {"Git.Git", "Git for Windows"};
const dp_package DP_DOCKER_DESKTOP = {"Docker.DockerDesktop", "Docker Desktop"};
const dp_package DP_PYTHON = {"Python.Python.3.13", "Python 3.13"};

/* winget's APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE: the package is
 * already installed and current. */
const int DP_WINGET_ALREADY_INSTALLED = (int)0x8A15002Bu;

static char *format(const char *fmt, ...) OMT_PRINTF(1, 2);
static char *format(const char *fmt, ...) {
    omt_buf b;
    omt_buf_init(&b, 1u << 20);
    va_list args;
    va_start(args, fmt);
    omt_buf_vprintf(&b, fmt, args);
    va_end(args);
    char *out = dp_strdup(omt_buf_cstr(&b));
    omt_buf_free(&b);
    return out;
}

/* --------------------------------------------------------------- lookup */

void dp_suffixes_free(dp_suffixes *s) {
    for (size_t i = 0; i < s->count; i++) free(s->items[i]);
    s->count = 0;
}

static bool suffix_known(const dp_suffixes *s, const char *entry, size_t len) {
    for (size_t i = 0; i < s->count; i++) {
        if (strlen(s->items[i]) == len && omt_ascii_ieq(entry, len, s->items[i])) return true;
    }
    return false;
}

void dp_executable_suffixes(bool windows, const char *pathext, dp_suffixes *out) {
    out->count = 0;
    out->items[out->count++] = dp_strdup("");
    if (!windows) return;
    const char *p = pathext ? pathext : "";
    while (*p && out->count < DP_MAX_SUFFIXES) {
        const char *semi = strchr(p, ';');
        size_t n = semi ? (size_t)(semi - p) : strlen(p);
        omt_span t = omt_utf8_trim(p, n);
        bool usable = t.len > 1 && t.p[0] == '.' && !memchr(t.p, '/', t.len) &&
                      !memchr(t.p, '\\', t.len) && !memchr(t.p, ' ', t.len);
        if (usable && !suffix_known(out, (const char *)t.p, t.len)) {
            out->items[out->count++] = dp_strndup((const char *)t.p, t.len);
        }
        p += n + (semi ? 1 : 0);
    }
    if (out->count == 1) {
        static const char *const fallback[] = {".COM", ".EXE", ".BAT", ".CMD"};
        for (size_t i = 0; i < 4; i++) out->items[out->count++] = dp_strdup(fallback[i]);
    }
}

static bool has_separator(const char *program) {
    return strchr(program, '/') || (DP_ON_WINDOWS && strchr(program, '\\'));
}

bool dp_find_executable(const char *program, omt_buf *out) {
    if (has_separator(program)) {
        if (!dp_is_executable_file(program)) return false;
        omt_buf_puts(out, program);
        return true;
    }
    omt_buf pathext, path, candidate;
    omt_buf_init(&pathext, 4096);
    omt_buf_init(&path, 1u << 20);
    omt_buf_init(&candidate, 8192);
    bool has_pathext = dp_getenv("PATHEXT", &pathext);
    dp_suffixes suffixes;
    dp_executable_suffixes(DP_ON_WINDOWS, has_pathext ? omt_buf_cstr(&pathext) : NULL, &suffixes);
    bool found = false;
    if (dp_getenv("PATH", &path)) {
        const char *p = omt_buf_cstr(&path);
        while (!found) {
            const char *sep = strchr(p, DP_PATH_LIST_SEP);
            size_t n = sep ? (size_t)(sep - p) : strlen(p);
            if (n > 0) {
                char *dir = dp_strndup(p, n);
                for (size_t i = 0; dir && i < suffixes.count && !found; i++) {
                    omt_buf_clear(&candidate);
                    omt_buf name;
                    omt_buf_init(&name, 8192);
                    omt_buf_printf(&name, "%s%s", program, suffixes.items[i]);
                    dp_path_join(&candidate, dir, omt_buf_cstr(&name));
                    omt_buf_free(&name);
                    if (dp_is_executable_file(omt_buf_cstr(&candidate))) {
                        omt_buf_puts(out, omt_buf_cstr(&candidate));
                        found = true;
                    }
                }
                free(dir);
            }
            if (!sep) break;
            p = sep + 1;
        }
    }
    dp_suffixes_free(&suffixes);
    omt_buf_free(&pathext);
    omt_buf_free(&path);
    omt_buf_free(&candidate);
    return found;
}

bool dp_is_wsl_launcher(const char *path) {
    omt_buf lowered;
    omt_buf_init(&lowered, 1u << 16);
    for (const char *p = path; *p; p++) {
        char c = *p == '\\' ? '/' : *p;
        omt_buf_putc(&lowered, (uint8_t)(c >= 'A' && c <= 'Z' ? c + 32 : c));
    }
    const char *text = omt_buf_cstr(&lowered);
    bool wsl = strstr(text, "/windows/system32/") || strstr(text, "/windows/sysnative/");
    omt_buf_free(&lowered);
    return wsl;
}

void dp_candidates_free(dp_candidates *c) {
    for (size_t i = 0; i < c->count; i++) free(c->items[i]);
    c->count = 0;
}

static void push_bash_root(dp_candidates *c, const char *root) {
    static const char *const tails[2][2] = {{"bin", "bash.exe"}, {"usr", "bin"}};
    for (int k = 0; k < 2 && c->count < DP_MAX_CANDIDATES; k++) {
        omt_buf a, b;
        omt_buf_init(&a, 8192);
        omt_buf_init(&b, 8192);
        dp_path_join(&a, root, tails[k][0]);
        if (k == 0) {
            dp_path_join(&b, omt_buf_cstr(&a), "bash.exe");
        } else {
            omt_buf mid;
            omt_buf_init(&mid, 8192);
            dp_path_join(&mid, omt_buf_cstr(&a), "bin");
            dp_path_join(&b, omt_buf_cstr(&mid), "bash.exe");
            omt_buf_free(&mid);
        }
        c->items[c->count++] = dp_strdup(omt_buf_cstr(&b));
        omt_buf_free(&a);
        omt_buf_free(&b);
    }
}

void dp_windows_bash_candidates(dp_env_lookup lookup, void *ctx, dp_candidates *out) {
    out->count = 0;
    omt_buf value, root;
    omt_buf_init(&value, 8192);
    omt_buf_init(&root, 8192);
    static const char *const program_files[] = {"ProgramFiles", "ProgramFiles(x86)",
                                                "ProgramW6432"};
    for (size_t i = 0; i < 3; i++) {
        omt_buf_clear(&value);
        omt_buf_clear(&root);
        if (lookup(ctx, program_files[i], &value)) {
            dp_path_join(&root, omt_buf_cstr(&value), "Git");
            push_bash_root(out, omt_buf_cstr(&root));
        }
    }
    omt_buf_clear(&value);
    if (lookup(ctx, "LOCALAPPDATA", &value)) {
        omt_buf programs;
        omt_buf_init(&programs, 8192);
        dp_path_join(&programs, omt_buf_cstr(&value), "Programs");
        omt_buf_clear(&root);
        dp_path_join(&root, omt_buf_cstr(&programs), "Git");
        push_bash_root(out, omt_buf_cstr(&root));
        omt_buf_free(&programs);
    }
    /* SystemDrive is `C:` with no separator, and a bare `C:` joined with a
     * relative path names the drive's current directory rather than its
     * root. */
    omt_buf drive;
    omt_buf_init(&drive, 8192);
    omt_buf_clear(&value);
    if (lookup(ctx, "SystemDrive", &value)) {
        omt_buf_printf(&drive, "%s\\", omt_buf_cstr(&value));
    } else {
        omt_buf_puts(&drive, "C:\\");
    }
    static const char *const roots[] = {"msys64", "Git"};
    for (size_t i = 0; i < 2; i++) {
        omt_buf_clear(&root);
        dp_path_join(&root, omt_buf_cstr(&drive), roots[i]);
        push_bash_root(out, omt_buf_cstr(&root));
    }
    omt_buf_free(&drive);
    omt_buf_free(&value);
    omt_buf_free(&root);
}

static bool env_lookup(void *ctx, const char *name, omt_buf *out) {
    (void)ctx;
    return dp_getenv(name, out) && out->len > 0;
}

bool dp_find_bash(omt_buf *out) {
    if (DP_ON_WINDOWS) {
        dp_candidates c;
        dp_windows_bash_candidates(env_lookup, NULL, &c);
        bool found = false;
        for (size_t i = 0; i < c.count && !found; i++) {
            if (dp_is_executable_file(c.items[i])) {
                omt_buf_puts(out, c.items[i]);
                found = true;
            }
        }
        dp_candidates_free(&c);
        if (found) return true;
    }
    omt_buf found;
    omt_buf_init(&found, 8192);
    bool ok = dp_find_executable("bash", &found) &&
              (!DP_ON_WINDOWS || !dp_is_wsl_launcher(omt_buf_cstr(&found)));
    if (ok) omt_buf_puts(out, omt_buf_cstr(&found));
    omt_buf_free(&found);
    return ok;
}

/* Docker first, then Podman: the order scripts/docker-test-env.sh uses. */
bool dp_find_container_engine(omt_buf *out, const char **kind) {
    if (dp_find_executable("docker", out)) {
        *kind = "docker";
        return true;
    }
    omt_buf_clear(out);
    if (dp_find_executable("podman", out)) {
        *kind = "podman";
        return true;
    }
    return false;
}

/* ------------------------------------------------------------ the plan */

void dp_build_plan_free(dp_build_plan *plan) {
    free(plan->program);
    for (size_t i = 0; i < 4; i++) free(plan->args[i]);
    for (size_t i = 0; i < 2; i++) free(plan->env[i]);
    memset(plan, 0, sizeof(*plan));
}

void dp_build_plan_summary(const dp_build_plan *plan, omt_buf *out) {
    omt_buf_puts(out, plan->program);
    for (size_t i = 0; i < 4 && plan->args[i]; i++) {
        omt_buf_putc(out, ' ');
        omt_buf_puts(out, plan->args[i]);
    }
}

/* Windows goes through bash even when GNU Make is installed: the Makefile
 * recipe is a call to scripts/build-arm64.sh, so make without a POSIX shell
 * hands that script to cmd.exe and fails. That makes Git for Windows the only
 * shell prerequisite on Windows, and GNU Make none at all. */
bool dp_plan_from(const char *make, const char *bash, bool windows, dp_build_plan *plan,
                  dp_err *err) {
    memset(plan, 0, sizeof(*plan));
    if (bash && (windows || !make)) {
        plan->program = dp_strdup(bash);
        plan->args[0] = dp_strdup("scripts/build-arm64.sh");
        plan->env[0] = dp_strdup("ARM64_TARBALL=" DP_IMAGE_MEMBER);
        return true;
    }
    if (windows) {
        dp_fail(err,
                "no POSIX shell was found, so the ARM64 appliance image cannot be rebuilt on this "
                "Windows machine. Install %s, or deploy without a project root: the archive "
                "embedded in this deployer needs no build tooling.",
                DP_GIT_FOR_WINDOWS.name);
        return false;
    }
    if (make) {
        plan->program = dp_strdup(make);
        plan->args[0] = dp_strdup("build-arm64");
        plan->args[1] = dp_strdup("ARM64_TARBALL=" DP_IMAGE_MEMBER);
        return true;
    }
    dp_fail(err, "neither GNU Make nor bash is on PATH, so the ARM64 appliance image cannot be "
                 "rebuilt. Install them with `make install`, or deploy without a project root: "
                 "the archive embedded in this deployer needs no build tooling.");
    return false;
}

bool dp_image_build_plan(dp_build_plan *plan, dp_err *err) {
    omt_buf make, bash;
    omt_buf_init(&make, 8192);
    omt_buf_init(&bash, 8192);
    bool has_make = dp_find_executable("make", &make);
    bool has_bash = dp_find_bash(&bash);
    bool ok = dp_plan_from(has_make ? omt_buf_cstr(&make) : NULL,
                           has_bash ? omt_buf_cstr(&bash) : NULL, DP_ON_WINDOWS, plan, err);
    omt_buf_free(&make);
    omt_buf_free(&bash);
    return ok;
}

/* ---------------------------------------------------------- the report */

static void add_row(dp_prerequisites *rows, const char *name, const char *purpose, bool required,
                    bool satisfied, char *detail, char *remedy, const dp_package *package) {
    dp_prerequisite *grown = realloc(rows->rows, (rows->count + 1) * sizeof(*grown));
    if (!grown) {
        free(detail);
        free(remedy);
        return;
    }
    rows->rows = grown;
    dp_prerequisite *row = &rows->rows[rows->count++];
    row->name = name;
    row->purpose = purpose;
    row->required = required;
    row->satisfied = satisfied;
    row->detail = detail ? detail : dp_strdup("");
    row->remedy = remedy ? remedy : dp_strdup("");
    /* A winget package is only an answer on Windows. */
    row->package = satisfied || !DP_ON_WINDOWS ? NULL : package;
}

void dp_prerequisites_free(dp_prerequisites *rows) {
    for (size_t i = 0; i < rows->count; i++) {
        free(rows->rows[i].detail);
        free(rows->rows[i].remedy);
    }
    free(rows->rows);
    rows->rows = NULL;
    rows->count = 0;
}

static char *manual_remedy(const char *windows, const char *unix) {
    return dp_strdup(DP_ON_WINDOWS ? windows : unix);
}

static void shell_row(dp_prerequisites *rows) {
    omt_buf bash;
    omt_buf_init(&bash, 8192);
    if (dp_find_bash(&bash)) {
        add_row(rows, "POSIX shell", "runs the pinned ARM64 image build", DP_ON_WINDOWS, true,
                dp_strdup(omt_buf_cstr(&bash)), NULL, NULL);
    } else {
        add_row(rows, "POSIX shell", "runs the pinned ARM64 image build", DP_ON_WINDOWS, false,
                dp_strdup("no bash was found"),
                manual_remedy("Install Git for Windows. Its bash and coreutils are what the image "
                              "build runs in.",
                              "Install bash through this system's package manager."),
                &DP_GIT_FOR_WINDOWS);
    }
    omt_buf_free(&bash);
}

static void engine_row(dp_prerequisites *rows, const dp_cancel *cancel) {
    static const char name[] = "Container engine";
    static const char purpose[] = "builds and exports the ARM64 appliance image";
    omt_buf engine, cwd;
    omt_buf_init(&engine, 8192);
    omt_buf_init(&cwd, 8192);
    const char *kind = NULL;
    if (!dp_find_container_engine(&engine, &kind)) {
        add_row(rows, name, purpose, true, false, dp_strdup("neither docker nor podman was found"),
                manual_remedy("Install Docker Desktop and start it with the Linux engine selected.",
                              "Install Docker or Podman through this system's package manager."),
                &DP_DOCKER_DESKTOP);
        omt_buf_free(&engine);
        omt_buf_free(&cwd);
        return;
    }
    /* Installed is not running: Docker Desktop is the common case of a docker
     * on PATH whose engine is stopped. */
    dp_current_dir(&cwd);
    static const char *const info[] = {"info", NULL};
    dp_process_result result;
    dp_err err;
    dp_err_init(&err);
    bool ran = dp_run_process(omt_buf_cstr(&engine), info, omt_buf_cstr(&cwd), NULL, cancel,
                              &result, &err);
    if (ran && result.exit_code == 0) {
        add_row(rows, name, purpose, true, true,
                format("%s at %s is running", kind, omt_buf_cstr(&engine)), NULL, NULL);
    } else {
        add_row(rows, name, purpose, true, false,
                format("%s at %s is installed but not responding", kind, omt_buf_cstr(&engine)),
                manual_remedy("Start Docker Desktop and wait for it to report that the engine is "
                              "running.",
                              "Start the container engine's service, or add this account to its "
                              "group."),
                NULL);
    }
    if (ran) dp_process_result_free(&result);
    dp_err_free(&err);
    omt_buf_free(&engine);
    omt_buf_free(&cwd);
}

/* Python is reported as found wherever it resolves. Nothing breaks either way:
 * scripts/detect-version.sh falls back to the Git tag without it. */
static void python_row(dp_prerequisites *rows) {
    static const char purpose[] = "stamps the exact release version into the image";
    omt_buf python;
    omt_buf_init(&python, 8192);
    bool found = dp_find_executable("python3", &python) ||
                 (omt_buf_clear(&python), dp_find_executable("python", &python));
    if (found) {
        add_row(rows, "Python 3", purpose, false, true, dp_strdup(omt_buf_cstr(&python)), NULL,
                NULL);
    } else {
        add_row(rows, "Python 3", purpose, false, false, dp_strdup("no python3 was found"),
                dp_strdup("Optional. Without it the build falls back to the Git tag for its "
                          "version."),
                &DP_PYTHON);
    }
    omt_buf_free(&python);
}

static void project_rows(dp_prerequisites *rows, const char *root) {
    omt_buf deploy, manifest, archive;
    omt_buf_init(&deploy, 8192);
    omt_buf_init(&manifest, 8192);
    omt_buf_init(&archive, 8192);
    dp_path_join(&deploy, root, "deploy");
    dp_path_join(&manifest, omt_buf_cstr(&deploy), "manifest-v3.txt");
    if (dp_is_file(omt_buf_cstr(&manifest))) {
        add_row(rows, "Project source tree", "supplies the manifest-v3 capsule that is uploaded",
                true, true, dp_strdup(root), NULL, NULL);
    } else {
        add_row(rows, "Project source tree", "supplies the manifest-v3 capsule that is uploaded",
                true, false, format("%s is not there", omt_buf_cstr(&manifest)),
                dp_strdup("Point --project at the checkout of this repository, or drop it and "
                          "deploy the capsule embedded in this deployer."),
                NULL);
    }
    dp_path_join(&archive, root, DP_IMAGE_MEMBER);
    uint64_t bytes = 0;
    if (dp_file_size(omt_buf_cstr(&archive), &bytes)) {
        add_row(rows, "Appliance image archive", "the ARM64 image the Pi loads", false, true,
                format(DP_IMAGE_MEMBER ", %llu MiB", (unsigned long long)(bytes / (1024u * 1024u))),
                NULL, NULL);
    } else {
        add_row(rows, "Appliance image archive", "the ARM64 image the Pi loads", false, false,
                dp_strdup(DP_IMAGE_MEMBER " has not been built in this tree yet"),
                dp_strdup("Run `make build-arm64`, or deploy without --project to upload the "
                          "archive embedded in this deployer."),
                NULL);
    }
    omt_buf_free(&deploy);
    omt_buf_free(&manifest);
    omt_buf_free(&archive);
}

/* The only row an operator's deployment has, satisfied by construction: the
 * build will not produce a deployer without the archive. It is reported
 * because its size and digest are the only description of the appliance a
 * single-file deployer can offer. */
static void embedded_rows(dp_prerequisites *rows) {
    const dp_capsule_member *image = dp_capsule_image();
    char *detail;
    if (image) {
        size_t count;
        dp_capsule_members(&count);
        char digest[65];
        dp_sha256_hex(image->bytes, image->size, digest);
        detail = format("%zu files, " DP_IMAGE_MEMBER " %zu MiB, sha256 %.12s", count,
                        image->size / (1024u * 1024u), digest);
    } else {
        detail = dp_strdup(DP_IMAGE_MEMBER " is not embedded");
    }
    add_row(rows, "Embedded appliance capsule", "everything the Raspberry Pi receives", true,
            image != NULL, detail, NULL, NULL);
}

bool dp_probe_prerequisites(const char *project_root, const dp_cancel *cancel,
                            dp_prerequisites *out) {
    out->rows = NULL;
    out->count = 0;
    if (!project_root) {
        embedded_rows(out);
        return true;
    }
    engine_row(out, cancel);
    shell_row(out);
    if (!DP_ON_WINDOWS) {
        omt_buf make;
        omt_buf_init(&make, 8192);
        if (dp_find_executable("make", &make)) {
            add_row(out, "GNU Make", "the documented entry point for the image build", false, true,
                    dp_strdup(omt_buf_cstr(&make)), NULL, NULL);
        } else {
            add_row(out, "GNU Make", "the documented entry point for the image build", false, false,
                    dp_strdup("make is not on PATH"),
                    dp_strdup("Optional. Without it the build calls scripts/build-arm64.sh "
                              "directly."),
                    NULL);
        }
        omt_buf_free(&make);
    }
    python_row(out);
    project_rows(out, project_root);
    return true;
}

size_t dp_missing_packages(const dp_prerequisites *rows, const dp_package **out, size_t max) {
    size_t n = 0;
    for (size_t i = 0; i < rows->count; i++) {
        const dp_package *pkg = rows->rows[i].package;
        if (rows->rows[i].satisfied || !pkg) continue;
        bool seen = false;
        for (size_t j = 0; j < n; j++) seen |= out[j] == pkg;
        if (!seen && n < max) out[n++] = pkg;
    }
    return n;
}

bool dp_report_install(const dp_package *package, const dp_process_result *result,
                       const dp_progress *p, dp_err *err) {
    if (result->exit_code == 0) {
        dp_reportf(p, "Installed %s.", package->name);
        return true;
    }
    if (result->exit_code == DP_WINGET_ALREADY_INSTALLED) {
        dp_reportf(p, "%s is already installed.", package->name);
        return true;
    }
    omt_span t = omt_utf8_trim((const char *)result->output.data, result->output.len);
    dp_fail(err, "installing %s failed (winget exit %d):\n%.*s", package->name, result->exit_code,
            (int)t.len, (const char *)t.p);
    return false;
}

bool dp_install_packages(const dp_package *const *packages, size_t count, const dp_cancel *cancel,
                         const dp_progress *p, dp_err *err) {
    if (!DP_ON_WINDOWS) {
        dp_fail(err, "automatic prerequisite installation is Windows-only; on this system run "
                     "`make install`");
        return false;
    }
    if (count == 0) {
        dp_report(p, "Every prerequisite with a winget package is already installed.");
        return true;
    }
    omt_buf winget, cwd;
    omt_buf_init(&winget, 8192);
    omt_buf_init(&cwd, 8192);
    if (!dp_find_executable("winget", &winget)) {
        omt_buf_free(&winget);
        omt_buf_free(&cwd);
        dp_fail(err, "winget was not found. Install \"App Installer\" from the Microsoft Store, "
                     "then re-check prerequisites.");
        return false;
    }
    dp_current_dir(&cwd);
    dp_report(p, "Windows will ask to approve each installer. Approve the UAC prompt to continue.");
    bool ok = true;
    for (size_t i = 0; ok && i < count; i++) {
        dp_reportf(p, "Installing %s (%s)...", packages[i]->name, packages[i]->id);
        const char *args[] = {"install",
                              "--exact",
                              "--id",
                              packages[i]->id,
                              "--source",
                              "winget",
                              "--accept-package-agreements",
                              "--accept-source-agreements",
                              NULL};
        dp_process_result result;
        ok = dp_run_process(omt_buf_cstr(&winget), args, omt_buf_cstr(&cwd), NULL, cancel, &result,
                            err);
        if (ok) {
            ok = dp_report_install(packages[i], &result, p, err);
            dp_process_result_free(&result);
        }
    }
    if (ok) {
        dp_report(p, "Prerequisites installed. Docker Desktop must be started once, and a sign-out "
                     "or restart may be needed before new tools appear on PATH.");
    }
    omt_buf_free(&winget);
    omt_buf_free(&cwd);
    return ok;
}

/* ------------------------------------------------------------ emulation */

/* Whether an ARM64 probe ran, and ran on an ARM64 kernel. The machine name is
 * the last non-empty line: an engine that pulls the image first prints
 * progress ahead of the answer. */
bool dp_reports_aarch64(const dp_process_result *result) {
    if (result->exit_code != 0) return false;
    const char *text = omt_buf_cstr(&result->output);
    const char *last = NULL;
    size_t last_len = 0;
    const char *p = text;
    while (*p) {
        const char *nl = strchr(p, '\n');
        size_t n = nl ? (size_t)(nl - p) : strlen(p);
        omt_span t = omt_utf8_trim(p, n);
        if (t.len) {
            last = (const char *)t.p;
            last_len = t.len;
        }
        if (!nl) break;
        p = nl + 1;
    }
    return last && last_len == 7 && memcmp(last, "aarch64", 7) == 0;
}

void dp_emulation_failure(const char *kind, bool windows, bool repaired, const char *output,
                          omt_buf *out) {
    const char *remedy;
    if (!windows) {
        remedy = "Register it once with `make setup-arm64-emulation`, then re-check.";
    } else if (repaired) {
        remedy = "The emulator was registered from the pinned installer image and linux/arm64 "
                 "still will not run. Docker Desktop must be running its Linux engine, not "
                 "Windows containers: check Settings > General, then restart Docker Desktop and "
                 "re-check.";
    } else {
        remedy = "Start Docker Desktop's Linux engine, then re-check.";
    }
    omt_span t = omt_utf8_trim(output, strlen(output));
    omt_buf_printf(out, "%s cannot execute linux/arm64 containers here.\n%s", kind, remedy);
    if (t.len) {
        omt_buf_putc(out, '\n');
        omt_buf_append(out, t.p, t.len);
    }
}

static bool engine_run(const char *engine, const char *const *args, const dp_cancel *cancel,
                       dp_process_result *result, dp_err *err) {
    omt_buf cwd;
    omt_buf_init(&cwd, 8192);
    dp_current_dir(&cwd);
    bool ok = dp_run_process(engine, args, omt_buf_cstr(&cwd), NULL, cancel, result, err);
    omt_buf_free(&cwd);
    return ok;
}

/* `uname -m` is passed as the container's command rather than through
 * `--entrypoint /bin/sh`, matching scripts/check-arm64-emulation.sh: Git Bash
 * rewrites a leading-slash argument into a Windows path before docker.exe
 * ever sees it. */
static bool probe_arm64(const char *engine, const dp_cancel *cancel, dp_process_result *result,
                        dp_err *err) {
    static const char *const args[] = {
        "run", "--rm", "--platform", "linux/arm64", EMULATION_PROBE_IMAGE, "uname", "-m", NULL};
    return engine_run(engine, args, cancel, result, err);
}

bool dp_ensure_arm64_emulation(const dp_cancel *cancel, const dp_progress *p, dp_err *err) {
    omt_buf engine, message;
    omt_buf_init(&engine, 8192);
    omt_buf_init(&message, DP_ERR_LIMIT);
    const char *kind = NULL;
    bool ok = false;
    dp_process_result result;
    bool have_result = false;
    if (!dp_find_container_engine(&engine, &kind)) {
        dp_fail(err, "no container engine is installed, so ARM64 emulation cannot be checked");
        goto done;
    }
    if (strcmp(kind, "docker") == 0) {
        static const char *const args[] = {"buildx", "version", NULL};
        if (!engine_run(omt_buf_cstr(&engine), args, cancel, &result, err)) goto done;
        int code = result.exit_code;
        dp_process_result_free(&result);
        if (code != 0) {
            dp_fail(err, "Docker buildx is not available, so the ARM64 image cannot be exported");
            goto done;
        }
    }
    dp_report(p, "Running a pinned ARM64 container to confirm emulation is registered...");
    if (!probe_arm64(omt_buf_cstr(&engine), cancel, &result, err)) goto done;
    have_result = true;
    if (dp_reports_aarch64(&result)) {
        dp_report(p, "ARM64 emulation is registered on this workstation.");
        ok = true;
        goto done;
    }
    if (!DP_ON_WINDOWS) {
        dp_emulation_failure(kind, false, false, omt_buf_cstr(&result.output), &message);
        dp_fail(err, "%s", omt_buf_cstr(&message));
        goto done;
    }
    dp_report(p, "ARM64 emulation is not registered. Installing it into the container engine with "
                 "the pinned binfmt image...");
    dp_process_result_free(&result);
    have_result = false;
    static const char *const install[] = {
        "run", "--rm", "--privileged", BINFMT_INSTALLER_IMAGE, "--install", BINFMT_ARCHITECTURES,
        NULL};
    if (!engine_run(omt_buf_cstr(&engine), install, cancel, &result, err)) goto done;
    have_result = true;
    if (result.exit_code != 0) {
        omt_span t = omt_utf8_trim((const char *)result.output.data, result.output.len);
        dp_fail(err,
                "registering ARM64 emulation with %s failed (exit %d).\nDocker Desktop must be "
                "running its Linux engine for this to work.\n%.*s",
                kind, result.exit_code, (int)t.len, (const char *)t.p);
        goto done;
    }
    dp_process_result_free(&result);
    have_result = false;
    dp_report(p, "Re-running the pinned ARM64 container...");
    if (!probe_arm64(omt_buf_cstr(&engine), cancel, &result, err)) goto done;
    have_result = true;
    if (!dp_reports_aarch64(&result)) {
        dp_emulation_failure(kind, true, true, omt_buf_cstr(&result.output), &message);
        dp_fail(err, "%s", omt_buf_cstr(&message));
        goto done;
    }
    dp_report(p, "ARM64 emulation is registered. The container engine forgets this when it "
                 "restarts, so a deployment that builds the image registers it again.");
    ok = true;
done:
    if (have_result) dp_process_result_free(&result);
    omt_buf_free(&engine);
    omt_buf_free(&message);
    return ok;
}
