/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * rpi-omt-deploy: the scriptable deployer.
 *
 * Exit 2 is a usage failure that leaves the command unrun; exit 1 is a command
 * that ran and failed. Secrets arrive as JSON on stdin (--secrets-stdin) or
 * through terminal prompts (--interactive-secrets), never as arguments.
 * --json makes every output line one JSON object.
 */
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "common/version.h"
#include "deploy/core/deploy.h"

#define SECRETS_LIMIT (16u * 1024u)

typedef enum {
    CMD_NONE,
    CMD_CHECK,
    CMD_PREREQUISITES,
    CMD_SETUP_EMULATION,
    CMD_PREPARE_SD,
    CMD_ALPINE_SETUP,
    CMD_DEPLOY,
    CMD_STATUS,
    CMD_LOGS,
    CMD_RESTART,
    CMD_REBOOT,
    CMD_WEB_PASSWORD,
    CMD_HOSTNAME,
    CMD_WIFI,
} command;

static const struct {
    const char *name;
    command cmd;
    const char *about;
} COMMANDS[] = {
    {"check", CMD_CHECK, "Validate the capsule this deployment would upload"},
    {"prerequisites", CMD_PREREQUISITES,
     "Report the local tooling a deployment needs from this workstation"},
    {"setup-emulation", CMD_SETUP_EMULATION, "Make this workstation able to run ARM64 containers"},
    {"prepare-sd", CMD_PREPARE_SD,
     "Prepare a flashed Alpine boot partition for its first headless boot"},
    {"alpine-setup", CMD_ALPINE_SETUP,
     "Install Alpine in persistent sys mode on a factory Raspberry Pi image"},
    {"deploy", CMD_DEPLOY, "Upload, verify, install, and start the appliance"},
    {"status", CMD_STATUS, "Show the appliance container's status"},
    {"logs", CMD_LOGS, "Show the appliance's recent logs"},
    {"restart", CMD_RESTART, "Restart the appliance service"},
    {"reboot", CMD_REBOOT,
     "Reboot the Raspberry Pi operating system after acknowledging the request"},
    {"web-password", CMD_WEB_PASSWORD,
     "Change the Web GUI password and revoke every existing Web session"},
    {"hostname", CMD_HOSTNAME, "Rename an installed appliance, in the host and in the Web GUI"},
    {"wifi", CMD_WIFI, "Apply Wi-Fi settings on the appliance"},
};

typedef struct {
    /* global */
    const char *host, *username, *key, *known_hosts, *project;
    uint16_t port;
    bool json, secrets_stdin, interactive_secrets;
    command cmd;
    /* per command */
    bool install, check_emulation;
    const char *hostname, *ssid, *boot_directory, *country, *remote_directory, *name;
    bool rebuild_image, no_connect, replace_existing_profiles;
} cli;

typedef struct {
    dp_secret password, key_passphrase, sudo_password, bootstrap_root_password;
    dp_secret wifi_password, web_password, root_password, pi_password;
} secret_input;

static const char *const SECRET_FIELDS[] = {
    "password",      "key_passphrase", "sudo_password", "bootstrap_root_password",
    "wifi_password", "web_password",   "root_password", "pi_password",
};

static dp_secret *secret_slot(secret_input *s, size_t i) {
    dp_secret *slots[] = {
        &s->password,      &s->key_passphrase, &s->sudo_password, &s->bootstrap_root_password,
        &s->wifi_password, &s->web_password,   &s->root_password, &s->pi_password};
    return slots[i];
}

/* ------------------------------------------------------------------ output */

static void emit(bool json, const char *event, const char *message, int success) {
    if (!json) {
        printf("%s\n", message);
        fflush(stdout);
        return;
    }
    omt_buf line;
    omt_buf_init(&line, DP_ERR_LIMIT * 2);
    omt_buf_puts(&line, "{\"event\":");
    omt_json_write_cstr(&line, event);
    omt_buf_puts(&line, ",\"message\":");
    omt_json_write_cstr(&line, message);
    omt_buf_puts(&line, ",\"success\":");
    omt_buf_puts(&line, success < 0 ? "null" : success ? "true" : "false");
    omt_buf_putc(&line, '}');
    printf("%s\n", line.failed ? "{\"event\":\"error\"}" : omt_buf_cstr(&line));
    fflush(stdout);
    omt_buf_free(&line);
}

static void progress_line(void *ctx, const char *text) {
    emit(*(const bool *)ctx, "progress", text, -1);
}

/* ------------------------------------------------------------------- usage */

static void print_help(FILE *out, command cmd) {
    if (cmd == CMD_NONE) {
        fprintf(out, "Secure Raspberry Pi OMT appliance deployment client\n\n"
                     "Usage: rpi-omt-deploy [OPTIONS] <COMMAND>\n\nCommands:\n");
        for (size_t i = 0; i < OMT_ARRAY_LEN(COMMANDS); i++) {
            fprintf(out, "  %-16s %s\n", COMMANDS[i].name, COMMANDS[i].about);
        }
        fprintf(out, "  %-16s %s\n", "help", "Print this message or the help of a command");
    } else {
        for (size_t i = 0; i < OMT_ARRAY_LEN(COMMANDS); i++) {
            if (COMMANDS[i].cmd == cmd) {
                fprintf(out, "%s\n\nUsage: rpi-omt-deploy %s [OPTIONS]\n", COMMANDS[i].about,
                        COMMANDS[i].name);
            }
        }
        fprintf(out, "\nCommand options:\n");
        switch (cmd) {
        case CMD_PREREQUISITES:
            fprintf(out, "      --install            Install missing prerequisites with winget "
                         "(Windows only)\n"
                         "      --check-emulation    Also prove the engine runs ARM64 "
                         "containers\n");
            break;
        case CMD_PREPARE_SD:
            fprintf(out,
                    "      --boot-directory <PATH>  Mounted root of the Alpine boot partition\n"
                    "      --ssid <SSID>\n"
                    "      --country <CC>           Wi-Fi regulatory country [default: US]\n");
            break;
        case CMD_ALPINE_SETUP:
            fprintf(out, "      --hostname <NAME>\n      --ssid <SSID>\n");
            break;
        case CMD_DEPLOY:
            fprintf(out, "      --remote-directory <PATH>  [default: /opt/omt-client]\n"
                         "      --rebuild-image            Rebuild the ARM64 image from "
                         "--project first\n");
            break;
        case CMD_HOSTNAME:
            fprintf(out, "      --name <NAME>    One DNS label, 1-63 characters\n");
            break;
        case CMD_WIFI:
            fprintf(out, "      --ssid <SSID>\n      --no-connect\n"
                         "      --replace-existing-profiles\n");
            break;
        default: fprintf(out, "      (none)\n"); break;
        }
    }
    fprintf(out, "\nOptions:\n"
                 "      --host <HOST>\n"
                 "      --username <USERNAME>\n"
                 "      --port <PORT>                [default: 22]\n"
                 "      --key <KEY>\n"
                 "      --known-hosts <KNOWN_HOSTS>\n"
                 "      --project <PROJECT>          Take the capsule from this checkout\n"
                 "      --json\n"
                 "      --secrets-stdin\n"
                 "      --interactive-secrets\n"
                 "  -h, --help                       Print help\n"
                 "  -V, --version                    Print version\n");
}

static int usage_error(const char *fmt, ...) OMT_PRINTF(1, 2);
static int usage_error(const char *fmt, ...) {
    fprintf(stderr, "error: ");
    va_list args;
    va_start(args, fmt);
    vfprintf(stderr, fmt, args);
    va_end(args);
    fprintf(stderr, "\n\nUsage: rpi-omt-deploy [OPTIONS] <COMMAND>\n\nFor more information, "
                    "try '--help'.\n");
    return 2;
}

/* ----------------------------------------------------------------- parsing */

typedef enum { OPT_FLAG, OPT_VALUE } opt_kind;

typedef struct {
    const char *name;
    opt_kind kind;
    command scope; /* CMD_NONE for global options */
    size_t offset; /* into cli: a bool or a const char * */
} option;

#define FIELD(f) offsetof(cli, f)

static const option OPTIONS[] = {
    {"host", OPT_VALUE, CMD_NONE, FIELD(host)},
    {"username", OPT_VALUE, CMD_NONE, FIELD(username)},
    {"key", OPT_VALUE, CMD_NONE, FIELD(key)},
    {"known-hosts", OPT_VALUE, CMD_NONE, FIELD(known_hosts)},
    {"project", OPT_VALUE, CMD_NONE, FIELD(project)},
    {"json", OPT_FLAG, CMD_NONE, FIELD(json)},
    {"secrets-stdin", OPT_FLAG, CMD_NONE, FIELD(secrets_stdin)},
    {"interactive-secrets", OPT_FLAG, CMD_NONE, FIELD(interactive_secrets)},
    {"install", OPT_FLAG, CMD_PREREQUISITES, FIELD(install)},
    {"check-emulation", OPT_FLAG, CMD_PREREQUISITES, FIELD(check_emulation)},
    {"boot-directory", OPT_VALUE, CMD_PREPARE_SD, FIELD(boot_directory)},
    {"ssid", OPT_VALUE, CMD_PREPARE_SD, FIELD(ssid)},
    {"country", OPT_VALUE, CMD_PREPARE_SD, FIELD(country)},
    {"hostname", OPT_VALUE, CMD_ALPINE_SETUP, FIELD(hostname)},
    {"ssid", OPT_VALUE, CMD_ALPINE_SETUP, FIELD(ssid)},
    {"remote-directory", OPT_VALUE, CMD_DEPLOY, FIELD(remote_directory)},
    {"rebuild-image", OPT_FLAG, CMD_DEPLOY, FIELD(rebuild_image)},
    {"name", OPT_VALUE, CMD_HOSTNAME, FIELD(name)},
    {"ssid", OPT_VALUE, CMD_WIFI, FIELD(ssid)},
    {"no-connect", OPT_FLAG, CMD_WIFI, FIELD(no_connect)},
    {"replace-existing-profiles", OPT_FLAG, CMD_WIFI, FIELD(replace_existing_profiles)},
};

static bool *flag_at(cli *c, size_t offset) { return (bool *)((char *)c + offset); }
static const char **value_at(cli *c, size_t offset) { return (const char **)((char *)c + offset); }

/* Returns -1 to continue, or an exit status. */
static int parse(int argc, char **argv, cli *c) {
    memset(c, 0, sizeof(*c));
    c->port = 22;
    const char *port_text = NULL;
    bool seen[OMT_ARRAY_LEN(OPTIONS)] = {false};
    bool port_seen = false, help = false;
    for (int i = 1; i < argc; i++) {
        const char *arg = argv[i];
        if (strcmp(arg, "--help") == 0 || strcmp(arg, "-h") == 0) {
            help = true;
            continue;
        }
        if (strcmp(arg, "--version") == 0 || strcmp(arg, "-V") == 0) {
            printf("rpi-omt-deploy %s\n", omt_version);
            return 0;
        }
        if (arg[0] == '-' && arg[1] == '-' && arg[2]) {
            const char *name = arg + 2;
            const char *eq = strchr(name, '=');
            size_t name_len = eq ? (size_t)(eq - name) : strlen(name);
            if (name_len == 4 && memcmp(name, "port", 4) == 0) {
                if (port_seen)
                    return usage_error(
                        "the argument '--port <PORT>' cannot be used multiple times");
                port_seen = true;
                if (eq) {
                    port_text = eq + 1;
                } else if (i + 1 < argc) {
                    port_text = argv[++i];
                } else {
                    return usage_error(
                        "a value is required for '--port <PORT>' but none was supplied");
                }
                continue;
            }
            const option *match = NULL;
            size_t index = 0;
            for (size_t k = 0; k < OMT_ARRAY_LEN(OPTIONS); k++) {
                if (strlen(OPTIONS[k].name) != name_len ||
                    memcmp(OPTIONS[k].name, name, name_len) != 0)
                    continue;
                if (OPTIONS[k].scope == CMD_NONE || OPTIONS[k].scope == c->cmd) {
                    match = &OPTIONS[k];
                    index = k;
                    break;
                }
            }
            if (!match) return usage_error("unexpected argument '%s' found", arg);
            if (seen[index])
                return usage_error("the argument '%s' cannot be used multiple times", arg);
            seen[index] = true;
            if (match->kind == OPT_FLAG) {
                if (eq) return usage_error("unexpected value for '%s'", arg);
                *flag_at(c, match->offset) = true;
            } else if (eq) {
                *value_at(c, match->offset) = eq + 1;
            } else if (i + 1 < argc) {
                *value_at(c, match->offset) = argv[++i];
            } else {
                return usage_error("a value is required for '%s' but none was supplied", arg);
            }
            continue;
        }
        if (c->cmd == CMD_NONE && arg[0] != '-') {
            if (strcmp(arg, "help") == 0) {
                print_help(stdout, CMD_NONE);
                return 0;
            }
            for (size_t k = 0; k < OMT_ARRAY_LEN(COMMANDS); k++) {
                if (strcmp(arg, COMMANDS[k].name) == 0) c->cmd = COMMANDS[k].cmd;
            }
            if (c->cmd == CMD_NONE) return usage_error("unrecognized subcommand '%s'", arg);
            continue;
        }
        return usage_error("unexpected argument '%s' found", arg);
    }
    if (help) {
        print_help(stdout, c->cmd);
        return 0;
    }
    if (c->cmd == CMD_NONE) {
        print_help(stderr, CMD_NONE);
        return 2;
    }
    if (port_text) {
        uint64_t port;
        if (!omt_parse_u64(port_text, strlen(port_text), 65535, &port)) {
            return usage_error("invalid value '%s' for '--port <PORT>'", port_text);
        }
        c->port = (uint16_t)port;
    }
    if (c->secrets_stdin && c->interactive_secrets) {
        return usage_error("the argument '--secrets-stdin' cannot be used with "
                           "'--interactive-secrets'");
    }
    if (c->rebuild_image && !c->project) {
        return usage_error("the following required arguments were not provided:\n  "
                           "--project <PROJECT>");
    }
    const char *missing = NULL;
    if (c->cmd == CMD_ALPINE_SETUP && !c->hostname) missing = "--hostname <HOSTNAME>";
    if (c->cmd == CMD_PREPARE_SD && !c->boot_directory)
        missing = "--boot-directory <BOOT_DIRECTORY>";
    if ((c->cmd == CMD_PREPARE_SD || c->cmd == CMD_WIFI) && !c->ssid) missing = "--ssid <SSID>";
    if (c->cmd == CMD_HOSTNAME && !c->name) missing = "--name <NAME>";
    if (missing) {
        return usage_error("the following required arguments were not provided:\n  %s", missing);
    }
    if (!c->country) c->country = "US";
    if (!c->remote_directory) c->remote_directory = "/opt/omt-client";
    return -1;
}

/* ----------------------------------------------------------------- secrets */

static void secrets_init(secret_input *s) {
    for (size_t i = 0; i < OMT_ARRAY_LEN(SECRET_FIELDS); i++) dp_secret_init(secret_slot(s, i));
}

static void secrets_free(secret_input *s) {
    for (size_t i = 0; i < OMT_ARRAY_LEN(SECRET_FIELDS); i++) dp_secret_clear(secret_slot(s, i));
}

/* Reads the bounded --secrets-stdin channel. Every buffer that held the
 * document is wiped when freed: it carries every secret at once. */
static bool read_secrets(bool enabled, secret_input *s, dp_err *err) {
    if (!enabled) return true;
    omt_buf input;
    omt_buf_init(&input, SECRETS_LIMIT + 1);
    bool too_long = false;
    bool ok = dp_read_stdin(SECRETS_LIMIT, &input, &too_long, err);
    if (ok && too_long) {
        dp_fail(err, "secrets JSON exceeds 16 KiB");
        ok = false;
    }
    omt_json_doc doc = {0};
    const omt_json *root = NULL;
    if (ok) {
        root = omt_json_parse(&doc, (const char *)input.data, input.len, 0);
        if (!root || root->type != OMT_JSON_OBJECT) {
            dp_fail(err, "invalid secrets JSON: %s", root ? "expected an object" : doc.error);
            ok = false;
        } else if (!omt_json_only_keys(root, SECRET_FIELDS, OMT_ARRAY_LEN(SECRET_FIELDS))) {
            dp_fail(err, "invalid secrets JSON: unknown field");
            ok = false;
        }
    }
    for (size_t i = 0; ok && i < OMT_ARRAY_LEN(SECRET_FIELDS); i++) {
        const omt_json *value = omt_json_get(root, SECRET_FIELDS[i]);
        if (!value || value->type == OMT_JSON_NULL) continue;
        if (value->type != OMT_JSON_STRING) {
            dp_fail(err, "invalid secrets JSON: %s must be a string", SECRET_FIELDS[i]);
            ok = false;
            break;
        }
        const char *problem = dp_secret_assign(secret_slot(s, i), value->string, value->string_len);
        if (problem) {
            dp_fail(err, "%s", problem);
            ok = false;
        }
    }
    omt_json_doc_free_secret(&doc);
    omt_buf_free_secret(&input);
    return ok;
}

/* Prompts into `slot`; `exit_code` says how a failure exits. */
static bool prompt(const char *text, dp_secret *slot, int *exit_code, dp_err *err) {
    omt_buf value;
    omt_buf_init(&value, 64u * 1024u);
    bool ok = dp_prompt_secret(text, &value, err);
    if (!ok) {
        *exit_code = 1;
    } else {
        const char *problem = dp_secret_assign(slot, (const char *)value.data, value.len);
        if (problem) {
            dp_fail(err, "%s", problem);
            *exit_code = 2;
            ok = false;
        }
    }
    omt_buf_free_secret(&value);
    return ok;
}

/* The secret from stdin, or a prompt, or a usage error naming the field. */
static bool require_secret(const cli *c, dp_secret *slot, const char *field, const char *question,
                           int *exit_code, dp_err *err) {
    if (slot->set) return true;
    if (c->interactive_secrets) return prompt(question, slot, exit_code, err);
    dp_fail(err, "%s is required through --secrets-stdin or an interactive prompt", field);
    *exit_code = 2;
    return false;
}

static bool build_connection(const cli *c, secret_input *s, dp_connection *conn, int *exit_code,
                             dp_err *err) {
    *exit_code = 2;
    if (!c->host) {
        dp_fail(err, "--host is required");
        return false;
    }
    if (!c->username) {
        dp_fail(err, "--username is required");
        return false;
    }
    if (c->interactive_secrets && !c->key && !s->password.set &&
        !prompt("SSH password: ", &s->password, exit_code, err)) {
        return false;
    }
    if (c->interactive_secrets && !s->sudo_password.set) {
        if (!prompt("sudo password (empty if passwordless): ", &s->sudo_password, exit_code, err)) {
            return false;
        }
        if (dp_secret_len(&s->sudo_password) == 0) dp_secret_clear(&s->sudo_password);
    }
    if (c->interactive_secrets && !s->bootstrap_root_password.set) {
        if (!prompt("root password for clean-Alpine bootstrap (empty if not needed): ",
                    &s->bootstrap_root_password, exit_code, err)) {
            return false;
        }
        if (dp_secret_len(&s->bootstrap_root_password) == 0) {
            dp_secret_clear(&s->bootstrap_root_password);
        }
    }
    *exit_code = 2;
    conn->host = dp_strdup(c->host);
    conn->username = dp_strdup(c->username);
    conn->port = c->port;
    conn->auth = c->key ? DP_AUTH_KEY : DP_AUTH_PASSWORD;
    conn->key_path = c->key ? dp_strdup(c->key) : NULL;
    conn->known_hosts_path = c->known_hosts ? dp_strdup(c->known_hosts) : NULL;
    dp_secret_copy(&conn->password, &s->password);
    dp_secret_copy(&conn->key_passphrase, &s->key_passphrase);
    dp_secret_copy(&conn->sudo_password, &s->sudo_password);
    dp_secret_copy(&conn->bootstrap_root_password, &s->bootstrap_root_password);
    const char *problem = dp_validate_connection(conn);
    if (problem) {
        dp_fail(err, "%s", problem);
        return false;
    }
    return true;
}

static bool needs_connection(command cmd) {
    return cmd != CMD_CHECK && cmd != CMD_PREREQUISITES && cmd != CMD_SETUP_EMULATION &&
           cmd != CMD_PREPARE_SD;
}

static void prerequisite_line(const dp_prerequisite *row, omt_buf *out) {
    const char *mark = row->satisfied ? "ok" : row->required ? "MISSING" : "optional";
    omt_buf_printf(out, "[%s] %s: %s", mark, row->name, row->detail);
    if (!row->satisfied && row->remedy[0]) omt_buf_printf(out, " -- %s", row->remedy);
}

/* ----------------------------------------------------------------- commands */

static int run(const cli *c, dp_err *err) {
    secret_input s;
    secrets_init(&s);
    dp_connection conn;
    dp_connection_init(&conn);
    int code = 0;
    dp_cancel cancel = false;
    bool json = c->json;
    dp_progress p = {progress_line, &json};
    omt_buf text;
    omt_buf_init(&text, DP_ERR_LIMIT);

    if (!read_secrets(c->secrets_stdin, &s, err)) {
        code = 2;
        goto done;
    }
    if (needs_connection(c->cmd) && !build_connection(c, &s, &conn, &code, err)) goto done;
    code = 1;
    switch (c->cmd) {
    case CMD_CHECK:
        if (c->project) {
            omt_buf deploy_dir, path;
            omt_buf_init(&deploy_dir, 8192);
            omt_buf_init(&path, 8192);
            dp_path_join(&deploy_dir, c->project, "deploy");
            dp_path_join(&path, omt_buf_cstr(&deploy_dir), "manifest-v3.txt");
            dp_manifest m;
            bool ok = dp_load_manifest(omt_buf_cstr(&path), &m, err);
            omt_buf_free(&deploy_dir);
            omt_buf_free(&path);
            if (!ok) goto done;
            omt_buf_printf(&text, "Project capsule passed local validation: %zu members in %s.",
                           m.count, c->project);
            dp_manifest_free(&m);
        } else if (!dp_capsule_report(&text, err)) {
            goto done;
        }
        emit(json, "result", omt_buf_cstr(&text), 1);
        break;
    case CMD_PREREQUISITES: {
        dp_prerequisites rows;
        if (c->install) {
            dp_probe_prerequisites(c->project, &cancel, &rows);
            const dp_package *missing[8];
            size_t n = dp_missing_packages(&rows, missing, 8);
            dp_prerequisites_free(&rows);
            if (!dp_install_packages(missing, n, &cancel, &p, err)) goto done;
        }
        /* Probed after any installation, so the report describes the machine
         * as it now is. */
        dp_probe_prerequisites(c->project, &cancel, &rows);
        size_t blocking = 0;
        for (size_t i = 0; i < rows.count; i++) {
            omt_buf_clear(&text);
            prerequisite_line(&rows.rows[i], &text);
            emit(json, "progress", omt_buf_cstr(&text), -1);
            blocking += dp_prerequisite_blocking(&rows.rows[i]);
        }
        dp_prerequisites_free(&rows);
        if (c->check_emulation && !dp_ensure_arm64_emulation(&cancel, &p, err)) goto done;
        if (blocking > 0) {
            dp_fail(err, "%zu required workstation prerequisite(s) are missing%s", blocking,
                    DP_ON_WINDOWS && !c->install
                        ? "; rerun with --install to install the ones winget can supply"
                        : "");
            goto done;
        }
        emit(json, "result",
             c->project ? "This workstation can build and deploy the appliance."
                        : "This deployer can deploy the appliance it carries.",
             1);
        break;
    }
    case CMD_SETUP_EMULATION:
        /* On Windows there is no `make setup-arm64-emulation`: that target
         * installs a systemd binfmt unit on a Linux host. */
        if (DP_ON_WINDOWS) {
            if (!dp_ensure_arm64_emulation(&cancel, &p, err)) goto done;
            emit(json, "result", "This workstation can run ARM64 containers.", 1);
        } else {
            emit(json, "result", "Run `make setup-arm64-emulation` with administrator approval.",
                 1);
        }
        break;
    case CMD_PREPARE_SD: {
        if (!require_secret(c, &s.wifi_password, "wifi_password", "Wi-Fi password: ", &code, err)) {
            goto done;
        }
        dp_sd_settings sd = {
            dp_strdup(c->boot_directory), dp_strdup(c->country), dp_strdup(c->ssid), {false, {0}}};
        dp_secret_init(&sd.wifi_password);
        dp_secret_copy(&sd.wifi_password, &s.wifi_password);
        const char *problem = dp_validate_sd_settings(&sd, err);
        bool ok = !problem;
        if (problem) {
            if (problem != dp_err_text(err)) dp_fail(err, "%s", problem);
            code = 2;
        } else {
            ok = dp_prepare_sd_card(&sd, &cancel, &p, err);
        }
        dp_sd_settings_free(&sd);
        if (!ok) goto done;
        emit(json, "result", "The Alpine boot partition is ready for headless first boot.", 1);
        break;
    }
    case CMD_ALPINE_SETUP: {
        if (!require_secret(c, &s.root_password, "root_password", "Root password: ", &code, err) ||
            !require_secret(c, &s.pi_password, "pi_password", "pi password: ", &code, err)) {
            goto done;
        }
        if (c->ssid &&
            !require_secret(c, &s.wifi_password, "wifi_password", "Wi-Fi password: ", &code, err)) {
            goto done;
        }
        dp_alpine_settings a;
        memset(&a, 0, sizeof(a));
        a.hostname = dp_strdup(c->hostname);
        dp_secret_init(&a.root_password);
        dp_secret_init(&a.pi_password);
        dp_secret_copy(&a.root_password, &s.root_password);
        dp_secret_copy(&a.pi_password, &s.pi_password);
        if (c->ssid) {
            a.has_wifi = true;
            a.wifi.ssid = dp_strdup(c->ssid);
            dp_secret_init(&a.wifi.password);
            dp_secret_copy(&a.wifi.password, &s.wifi_password);
            a.wifi.connect = false;
            a.wifi.preserve_existing_profiles = true;
        }
        const char *problem = dp_validate_alpine_setup(&a);
        bool ok = !problem;
        if (problem) {
            dp_fail(err, "%s", problem);
            code = 2;
        } else {
            ok = dp_alpine_setup(&conn, &a, c->project, &cancel, &p, err);
        }
        dp_alpine_settings_free(&a);
        if (!ok) goto done;
        emit(json, "result", "Alpine sys-mode install completed successfully.", 1);
        break;
    }
    case CMD_DEPLOY: {
        dp_deploy_options o = {c->project ? dp_strdup(c->project) : NULL,
                               dp_strdup(c->remote_directory), c->rebuild_image};
        const char *problem = dp_validate_options(&o);
        bool ok = !problem;
        if (problem) {
            dp_fail(err, "%s", problem);
            code = 2;
        } else {
            ok = dp_deploy(&conn, &o, &cancel, &p, err);
        }
        dp_deploy_options_free(&o);
        if (!ok) goto done;
        emit(json, "result", "Deployment completed successfully.", 1);
        break;
    }
    case CMD_WIFI: {
        if (!require_secret(c, &s.wifi_password, "wifi_password", "Wi-Fi password: ", &code, err)) {
            goto done;
        }
        dp_wifi_settings w = {
            dp_strdup(c->ssid), {false, {0}}, !c->no_connect, !c->replace_existing_profiles};
        dp_secret_init(&w.password);
        dp_secret_copy(&w.password, &s.wifi_password);
        const char *problem = dp_validate_wifi(&w);
        bool ok = !problem;
        if (problem) {
            dp_fail(err, "%s", problem);
            code = 2;
        } else {
            ok = dp_apply_wifi(&conn, &w, &cancel, &p, err);
        }
        dp_wifi_settings_free(&w);
        if (!ok) goto done;
        emit(json, "result", "Wi-Fi settings applied.", 1);
        break;
    }
    case CMD_WEB_PASSWORD: {
        if (!s.web_password.set && c->interactive_secrets) {
            dp_secret confirmation;
            dp_secret_init(&confirmation);
            bool ok = prompt("New Web GUI password: ", &s.web_password, &code, err) &&
                      prompt("Confirm Web GUI password: ", &confirmation, &code, err);
            if (ok && (s.web_password.value.len != confirmation.value.len ||
                       memcmp(s.web_password.value.data, confirmation.value.data,
                              confirmation.value.len) != 0)) {
                dp_fail(err, "Web GUI password confirmation does not match");
                code = 2;
                ok = false;
            }
            dp_secret_clear(&confirmation);
            if (!ok) goto done;
        } else if (!require_secret(c, &s.web_password, "web_password", "", &code, err)) {
            goto done;
        }
        const char *problem = dp_validate_web_password(&s.web_password);
        if (problem) {
            dp_fail(err, "%s", problem);
            code = 2;
            goto done;
        }
        if (!dp_change_web_password(&conn, &s.web_password, &cancel, &p, err)) goto done;
        emit(json, "result", "Web GUI password changed.", 1);
        break;
    }
    case CMD_HOSTNAME:
        /* A usage error rather than a failed operation on the Pi. */
        if (!dp_valid_appliance_hostname(c->name)) {
            dp_fail(err, "--name must be one DNS label of 1-63 characters: letters, digits, and "
                         "inner hyphens");
            code = 2;
            goto done;
        }
        if (!dp_set_hostname(&conn, c->name, c->project, &cancel, &p, err)) goto done;
        omt_buf_printf(&text, "Appliance hostname changed to %s.", c->name);
        emit(json, "result", omt_buf_cstr(&text), 1);
        break;
    case CMD_STATUS:
    case CMD_LOGS:
    case CMD_RESTART:
    case CMD_REBOOT: {
        dp_action action = c->cmd == CMD_STATUS    ? DP_ACTION_STATUS
                           : c->cmd == CMD_LOGS    ? DP_ACTION_LOGS
                           : c->cmd == CMD_RESTART ? DP_ACTION_RESTART
                                                   : DP_ACTION_REBOOT;
        if (!dp_manage(&conn, action, &cancel, &p, err)) goto done;
        emit(json, "result", "Remote management action succeeded.", 1);
        break;
    }
    case CMD_NONE: break;
    }
    code = 0;
done:
    secrets_free(&s);
    dp_connection_free(&conn);
    omt_buf_free(&text);
    return code;
}

int main(int argc, char **argv) {
    cli c;
    int status = parse(argc, argv, &c);
    if (status >= 0) return status;
    dp_err err;
    dp_err_init(&err);
    if (!dp_sys_init(&err)) {
        fprintf(stderr, "%s\n", dp_err_text(&err));
        return 1;
    }
    int code = run(&c, &err);
    if (code != 0) {
        if (c.json) {
            emit(true, "error", dp_err_text(&err), 0);
        } else {
            fprintf(stderr, "%s\n", dp_err_text(&err));
        }
    }
    dp_err_free(&err);
    return code;
}
