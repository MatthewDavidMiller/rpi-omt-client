/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The deployer core, ported from the in-crate tests of omt-deployer-core
 * (lib.rs, ops.rs, tools.rs, jobs.rs, sd_card.rs, ssh.rs).
 */
#include <signal.h>
#include <sys/stat.h>
#include <unistd.h>

#include "deploy/core/deploy.h"
#include "deploy/core/ops_internal.h"
#include "deploy/core/tools_internal.h"
#include "deploy/ssh/ssh.h"
#include "test.h"

static void secret(dp_secret *s, const char *text) {
    dp_secret_init(s);
    if (dp_secret_assign_cstr(s, text)) {
        fprintf(stderr, "fixture secret refused: %s\n", text);
        exit(2);
    }
}

static void connection(dp_connection *c, const char *user) {
    dp_connection_init(c);
    c->host = dp_strdup("pi.local");
    c->username = dp_strdup(user);
}

/* ------------------------------------------------------------------ lib.rs */

static void validation_contract(void) {
    CHECK(dp_valid_host("pi.local"));
    CHECK(!dp_valid_host("-pi.local"));
    CHECK(dp_valid_username("pi_admin-1"));
    CHECK(dp_valid_appliance_hostname("omt-client"));
    CHECK(dp_valid_appliance_hostname("rpi5"));
    CHECK(!dp_valid_appliance_hostname("-pi"));
    CHECK(!dp_valid_appliance_hostname("omt_client"));
    CHECK(!dp_valid_appliance_hostname("omt.client"));
    CHECK(dp_valid_remote_directory("/opt/omt-client"));

    dp_connection empty_ssh;
    dp_connection_init(&empty_ssh);
    empty_ssh.host = dp_strdup("10.1.20.210");
    empty_ssh.username = dp_strdup("root");
    CHECK(dp_secret_assign_cstr(&empty_ssh.password, "") == NULL);
    CHECK(dp_validate_connection(&empty_ssh) == NULL);
    dp_connection_free(&empty_ssh);

    dp_secret s;
    secret(&s, "alpinepw");
    CHECK(dp_validate_os_password(&s) == NULL);
    CHECK(dp_secret_assign_cstr(&s, "short") == NULL);
    CHECK(dp_validate_os_password(&s) != NULL);
    CHECK(dp_secret_assign_cstr(&s, "correct horse battery staple") == NULL);
    CHECK(dp_validate_web_password(&s) == NULL);
    CHECK(dp_secret_assign_cstr(&s, "too-short") == NULL);
    CHECK(dp_validate_web_password(&s) != NULL);
    dp_secret_clear(&s);

    omt_buf q;
    omt_buf_init(&q, 256);
    dp_shell_quote(&q, "a'b");
    CHECK_STR(omt_buf_cstr(&q), "'a'\\''b'");
    omt_buf_free(&q);
}

static void secrets_refuse_controls_and_oversize(void) {
    dp_secret s;
    dp_secret_init(&s);
    CHECK(dp_secret_assign_cstr(&s, "a\x07"
                                    "b") != NULL);
    CHECK(!s.set);
    CHECK(dp_secret_assign_cstr(&s, "tab\tinside") != NULL);
    CHECK(dp_secret_assign(&s, "\xc2\x85", 2) != NULL); /* U+0085, a C1 control */
    CHECK(dp_secret_assign(&s, "\xff", 1) != NULL);     /* not UTF-8 */
    char *big = test_alloc(4098);
    memset(big, 'a', 4097);
    CHECK(dp_secret_assign(&s, big, 4096) == NULL);
    CHECK(dp_secret_assign(&s, big, 4097) != NULL);
    free(big);
    CHECK(dp_secret_assign_cstr(&s, "caf\xc3\xa9") == NULL);
    dp_secret_clear(&s);
}

static void restart_manages_the_service_even_before_first_container_creation(void) {
    const char *const *argv = dp_action_argv(DP_ACTION_RESTART);
    CHECK_STR(argv[0], "rc-service");
    CHECK_STR(argv[1], "omt-client");
    CHECK_STR(argv[2], "restart");
    CHECK(argv[3] == NULL);
}

static void reboot_is_a_fixed_deferred_host_action(void) {
    const char *const *argv = dp_action_argv(DP_ACTION_REBOOT);
    CHECK_STR(argv[0], "sh");
    CHECK_STR(argv[1], "-c");
    CHECK_STR(argv[2], "nohup sh -c 'sleep 1; /sbin/reboot' </dev/null >/dev/null 2>&1 &");
    CHECK(argv[3] == NULL);
}

static void hex_is_lower_case_and_fixed_width(void) {
    omt_buf b;
    omt_buf_init(&b, 1024);
    dp_hex_encode(&b, "", 0);
    CHECK_STR(omt_buf_cstr(&b), "");
    const uint8_t bytes[] = {0x00, 0x0f, 0xa5, 0xff};
    dp_hex_encode(&b, bytes, 4);
    CHECK_STR(omt_buf_cstr(&b), "000fa5ff");
    omt_buf_clear(&b);
    dp_hex_encode(&b, "studio", 6);
    CHECK_STR(omt_buf_cstr(&b), "73747564696f");
    omt_buf_free(&b);
}

static void psk_vector(void) {
    dp_secret pass, psk;
    secret(&pass, "password");
    dp_secret_init(&psk);
    CHECK(dp_derive_wpa_psk("IEEE", &pass, &psk) == NULL);
    CHECK_STR(dp_secret_text(&psk),
              "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
    dp_secret_clear(&pass);
    dp_secret_clear(&psk);
}

static void write_text(const char *path, const char *text) {
    FILE *f = fopen(path, "wb");
    if (!f) {
        fprintf(stderr, "cannot write %s\n", path);
        exit(2);
    }
    fputs(text, f);
    fclose(f);
}

static char scratch[256];

static const char *scratch_path(const char *name) {
    static char path[512];
    omt_snprintf(path, sizeof(path), "%s/%s", scratch, name);
    return path;
}

static void manifest_requires_transaction_members(void) {
    dp_manifest m;
    dp_err err;
    dp_err_init(&err);
    write_text(scratch_path("manifest-v3.txt"), "version=3\nLICENSE\n");
    CHECK(!dp_load_manifest(scratch_path("manifest-v3.txt"), &m, &err));
    write_text(scratch_path("manifest-v3.txt"),
               "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n");
    CHECK(dp_load_manifest(scratch_path("manifest-v3.txt"), &m, &err));
    CHECK_INT(m.count, 2);
    dp_manifest_free(&m);

    const char dup[] =
        "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\nLICENSE\nLICENSE\n";
    CHECK(!dp_parse_manifest(dup, sizeof(dup) - 1, &m, &err));
    CHECK_STR(dp_err_text(&err), "unsafe or duplicate manifest member");
    const char unsafe[] = "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n../etc\n";
    CHECK(!dp_parse_manifest(unsafe, sizeof(unsafe) - 1, &m, &err));
    const char crlf[] = "version=3\r\ndeploy/transaction.sh\r\ndeploy/manifest-v3.txt\r\n";
    CHECK(dp_parse_manifest(crlf, sizeof(crlf) - 1, &m, &err));
    dp_manifest_free(&m);
    CHECK(!dp_parse_manifest("version=2\n", 10, &m, &err));
    CHECK_STR(dp_err_text(&err), "unsupported manifest");
    dp_err_free(&err);
}

static void manifest_names_are_safe(void) {
    CHECK(dp_valid_manifest_name("deploy/host/install.sh"));
    CHECK(!dp_valid_manifest_name("/etc/passwd"));
    CHECK(!dp_valid_manifest_name("a//b"));
    CHECK(!dp_valid_manifest_name("a/./b"));
    CHECK(!dp_valid_manifest_name("a/../b"));
    CHECK(!dp_valid_manifest_name("trailing/"));
    CHECK(!dp_valid_manifest_name("sp ace"));
    CHECK(!dp_valid_manifest_name(""));
    CHECK(!dp_valid_remote_directory("opt/omt-client"));
    CHECK(!dp_valid_remote_directory("/opt/omt-client/"));
    CHECK(!dp_valid_remote_directory("/opt/../etc"));
    CHECK(!dp_valid_remote_directory("/"));
}

static void cancelled_process_is_not_spawned(void) {
    dp_cancel cancel = true;
    dp_process_result r;
    dp_err err;
    dp_err_init(&err);
    CHECK(!dp_run_process("/nonexistent/omt-command-test", NULL, ".", NULL, &cancel, &r, &err));
    CHECK_STR(dp_err_text(&err), "operation cancelled");
    dp_err_free(&err);
}

static void process_captures_both_streams_and_nonzero_exit(void) {
    const char *args[] = {"-c", "printf out; printf err >&2; exit 7", NULL};
    dp_process_result r;
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_run_process("/bin/sh", args, "/tmp", NULL, NULL, &r, &err));
    CHECK_INT(r.exit_code, 7);
    CHECK_STR(omt_buf_cstr(&r.output), "outerr");
    dp_process_result_free(&r);

    /* The environment additions reach the child, and the directory is used. */
    const char *env_args[] = {"-c", "printf '%s:' \"$OMT_PLAN\"; pwd", NULL};
    const char *env[] = {"OMT_PLAN=yes", NULL};
    CHECK(dp_run_process("/bin/sh", env_args, "/", env, NULL, &r, &err));
    CHECK_STR(omt_buf_cstr(&r.output), "yes:/\n");
    dp_process_result_free(&r);

    CHECK(!dp_run_process("/nonexistent/omt-tool", NULL, ".", NULL, NULL, &r, &err));
    CHECK(strstr(dp_err_text(&err), "cannot run /nonexistent/omt-tool") != NULL);
    dp_err_free(&err);
}

typedef struct {
    dp_cancel *flag;
} cancel_after;

static void cancel_later(void *raw) {
    cancel_after *c = raw;
    dp_sleep_ms(100);
    atomic_store(c->flag, true);
}

static void cancellation_remains_live_after_the_direct_child_exits(void) {
    dp_cancel cancel = false;
    cancel_after c = {&cancel};
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_thread_spawn(cancel_later, &c, &err));
    uint64_t started = dp_now_ms();
    const char *args[] = {"-c", "sleep 2 & exit 0", NULL};
    dp_process_result r;
    CHECK(!dp_run_process("/bin/sh", args, "/tmp", NULL, &cancel, &r, &err));
    CHECK_STR(dp_err_text(&err), "operation cancelled");
    CHECK(dp_now_ms() - started < 1000);
    dp_err_free(&err);
}

static void output_limit_keeps_the_first_four_mib(void) {
    const char *args[] = {"-c", "head -c 4202496 /dev/zero | tr '\\0' '*'", NULL};
    dp_process_result r;
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_run_process("/bin/sh", args, "/tmp", NULL, NULL, &r, &err));
    CHECK_INT(r.exit_code, 0);
    CHECK_INT(r.output.len, DP_OUTPUT_LIMIT);
    dp_process_result_free(&r);
    dp_err_free(&err);
}

/* ------------------------------------------------------------------ ops.rs */

static void probe(char *out, size_t size, const char *model) {
    omt_snprintf(out, size, "aarch64\nalpine\n3.24.2\n%s\n", model);
}

/* The same matrix as tests/unit/test_board_profile.sh: the two gates run on
 * different machines, so a board either passes both or fails halfway. */
static void accepts_every_supported_board(void) {
    const char *models[] = {"Raspberry Pi 5 Model B Rev 1.0", "Raspberry Pi 4 Model B Rev 1.4"};
    char text[256];
    for (size_t i = 0; i < 2; i++) {
        probe(text, sizeof(text), models[i]);
        CHECK_MSG(dp_require_supported_appliance(text, NULL), "rejected %s", models[i]);
    }
}

static void refuses_unsupported_boards_and_near_misses(void) {
    const char *models[] = {
        "Raspberry Pi 500 Rev 1.0",
        "Raspberry Pi 400 Rev 1.0",
        "Raspberry Pi 2 Model B Rev 1.1",
        "Raspberry Pi 3 Model B Rev 1.2",
        "Raspberry Pi 3 Model B Plus Rev 1.3",
        "Raspberry Pi 3 Model A Plus Rev 1.0",
        "Raspberry Pi Zero 2 W Rev 1.0",
        "Raspberry Pi Zero 2 Rev 1.0",
        "Raspberry Pi Zero W Rev 1.1",
        "Raspberry Pi Model B Plus Rev 1.2",
        "Raspberry Pi Compute Module 4 Rev 1.0",
        "Raspberry Pi Compute Module 5 Rev 1.0",
        "Orange Pi 5",
        "",
    };
    char text[256];
    dp_err err;
    dp_err_init(&err);
    for (size_t i = 0; i < OMT_ARRAY_LEN(models); i++) {
        probe(text, sizeof(text), models[i]);
        CHECK_MSG(!dp_require_supported_appliance(text, &err), "accepted %s", models[i]);
    }
    dp_err_free(&err);
}

static void still_refuses_the_wrong_architecture_or_distribution(void) {
    const char *outputs[] = {
        "armv7l\nalpine\n3.24.2\nRaspberry Pi 5 Model B Rev 1.0\n",
        "aarch64\ndebian\n3.24.2\nRaspberry Pi 5 Model B Rev 1.0\n",
        "aarch64\nalpine\n3.22.1\nRaspberry Pi 5 Model B Rev 1.0\n",
        /* 3.23 was the previously pinned series; package names moved in 3.24. */
        "aarch64\nalpine\n3.23.5\nRaspberry Pi 5 Model B Rev 1.0\n",
        "aarch64\nalpine\n3.24.2\n",
    };
    dp_err err;
    dp_err_init(&err);
    for (size_t i = 0; i < OMT_ARRAY_LEN(outputs); i++) {
        CHECK_MSG(!dp_require_supported_appliance(outputs[i], &err), "accepted %s", outputs[i]);
    }
    CHECK_STR(dp_err_text(&err), "remote host must run Alpine Linux 3.24 aarch64 on a Raspberry "
                                 "Pi 5 or Raspberry Pi 4 Model B");
    dp_err_free(&err);
}

static dp_host_tooling tooling(const char *uid, bool bash, bool sudo, bool doas) {
    dp_host_tooling t;
    omt_strlcpy(t.uid, uid, sizeof(t.uid));
    t.has_bash = bash;
    t.has_sudo = sudo;
    t.has_doas = doas;
    return t;
}

static void picks_an_escalation_for_each_stock_alpine_shape(void) {
    dp_err err;
    dp_err_init(&err);
    dp_host_tooling t = tooling("0", false, false, false);
    CHECK_STR(dp_bootstrap_escalation(&t, &err), "");
    t = tooling("1000", false, true, false);
    CHECK_STR(dp_bootstrap_escalation(&t, &err), "sudo -S -p ''");
    t = tooling("1000", false, false, true);
    CHECK_STR(dp_bootstrap_escalation(&t, &err), "doas");
    t = tooling("1000", false, false, false);
    CHECK(dp_bootstrap_escalation(&t, &err) == NULL);
    CHECK(strstr(dp_err_text(&err), "bootstrap_root_password") != NULL);
    dp_err_free(&err);
}

static void parses_the_tooling_probe(void) {
    dp_host_tooling t = dp_parse_host_tooling("1000\nno\nno\nyes\n");
    CHECK_STR(t.uid, "1000");
    CHECK(!t.has_bash);
    CHECK(!t.has_sudo);
    CHECK(t.has_doas);
    t = dp_parse_host_tooling(" 0 \r\nyes\r\nyes\r\nno");
    CHECK_STR(t.uid, "0");
    CHECK(t.has_bash && t.has_sudo && !t.has_doas);
}

static void reports_the_detected_board_for_progress(void) {
    char text[256], board[128];
    probe(text, sizeof(text), "Raspberry Pi 4 Model B Rev 1.4");
    CHECK(dp_probed_board(text, board, sizeof(board)));
    CHECK_STR(board, "Raspberry Pi 4 Model B Rev 1.4");
    CHECK(!dp_probed_board("aarch64\nalpine\n3.24.2\n", board, sizeof(board)));
}

static void digest_parser_accepts_sha256sum_output(void) {
    char digest[65];
    CHECK(dp_parse_sha256_line(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  file\n", digest));
    CHECK_STR(digest, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    CHECK(dp_parse_sha256_line(
        "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef  f", digest));
    CHECK_STR(digest, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    CHECK(!dp_parse_sha256_line("not-a-digest", digest));
}

static void redaction_avoids_infinite_loops_on_marker_substrings(void) {
    const char *secrets[] = {"secret", "eda"};
    omt_buf out;
    omt_buf_init(&out, 1024);
    dp_redact("secret [redacted] secret", secrets, 2, &out);
    CHECK(strstr(omt_buf_cstr(&out), "secret") == NULL);
    CHECK(strstr(omt_buf_cstr(&out), "[redacted]") != NULL);
    const char *none[] = {"", NULL};
    dp_redact("unchanged", none, 2, &out);
    CHECK_STR(omt_buf_cstr(&out), "unchanged");
    omt_buf_free(&out);
}

static void successful_installs_surface_only_the_final_summary(void) {
    omt_buf out;
    omt_buf_init(&out, 1024);
    CHECK(dp_installer_summary(
        "apk noise\n=== Installation Complete ===\nWeb UI: https://pi:5000\n", &out));
    CHECK_STR(omt_buf_cstr(&out), "=== Installation Complete ===\nWeb UI: https://pi:5000");
    omt_buf_clear(&out);
    CHECK(!dp_installer_summary("apk noise only", &out));
    omt_buf_free(&out);
}

static void sudo_input_is_newline_terminated_and_prefixes_follow_the_account(void) {
    dp_connection c;
    connection(&c, "pi");
    CHECK(dp_secret_assign_cstr(&c.sudo_password, "hunter2") == NULL);
    omt_buf b;
    omt_buf_init(&b, 1024);
    dp_sudo_input(&c, &b);
    CHECK_STR(omt_buf_cstr(&b), "hunter2\n");
    CHECK_STR(dp_sudo_prefix(&c), "sudo -S -p ''");
    omt_buf_clear(&b);
    dp_privileged_command(&c, "docker ps", &b);
    CHECK_STR(omt_buf_cstr(&b), "sudo -S -p '' docker ps");
    omt_buf_clear(&b);
    dp_privileged_stdin_command(&c, "sh wifi-update", &b);
    CHECK_STR(omt_buf_cstr(&b), "sudo -S -p '' sh wifi-update");

    /* No password means passwordless sudo: nothing on stdin, and the
     * non-interactive prefix. An empty one is the same as none. */
    dp_secret_clear(&c.sudo_password);
    omt_buf_clear(&b);
    dp_sudo_input(&c, &b);
    CHECK_INT(b.len, 0);
    CHECK_STR(dp_sudo_prefix(&c), "sudo -n");
    CHECK(dp_secret_assign_cstr(&c.sudo_password, "") == NULL);
    dp_sudo_input(&c, &b);
    CHECK_INT(b.len, 0);
    CHECK_STR(dp_sudo_prefix(&c), "sudo -n");

    /* A root session runs the fixed command directly. */
    free(c.username);
    c.username = dp_strdup("root");
    CHECK(dp_secret_assign_cstr(&c.sudo_password, "unused") == NULL);
    dp_sudo_input(&c, &b);
    CHECK_INT(b.len, 0);
    CHECK_STR(dp_sudo_prefix(&c), "");
    omt_buf_clear(&b);
    dp_privileged_command(&c, "docker ps", &b);
    CHECK_STR(omt_buf_cstr(&b), "docker ps");
    omt_buf_free_secret(&b);
    dp_connection_free(&c);
}

static void web_password_action_uses_only_a_fixed_stdin_command(void) {
    CHECK_STR(DP_WEB_PASSWORD_COMMAND,
              "sh -eu -c 'docker exec -i omt-client /usr/local/bin/omt-web set-password && "
              "rc-service omt-client restart'");
    CHECK(strstr(DP_WEB_PASSWORD_COMMAND, "$1") == NULL);
    CHECK(strstr(DP_WEB_PASSWORD_COMMAND, "printf") == NULL);
}

static void the_rename_command_names_only_the_uploaded_script(void) {
    omt_buf q, inner;
    omt_buf_init(&q, 256);
    omt_buf_init(&inner, 1024);
    dp_shell_quote(&q, "/tmp/omt-set-hostname-0123456789ab.sh");
    dp_ssh_rename_command(omt_buf_cstr(&q), &inner);
    const char *text = omt_buf_cstr(&inner);
    CHECK(strstr(text, "/tmp/omt-set-hostname-0123456789ab.sh") != NULL);
    CHECK(strstr(text, "studio-pi-2") == NULL);
    CHECK(strstr(text, "rm -f --") != NULL);
    size_t len = strlen(text);
    CHECK(len > 8 && strcmp(text + len - 8, "exit $rc") == 0);
    omt_buf_free(&q);
    omt_buf_free(&inner);
}

static void untouched_alpine_can_bootstrap_through_su_only_with_a_root_secret(void) {
    dp_host_tooling t = tooling("1000", false, false, false);
    dp_connection c;
    connection(&c, "pi");
    CHECK(dp_secret_assign_cstr(&c.password, "ssh-password") == NULL);
    CHECK(dp_secret_assign_cstr(&c.sudo_password, "user-password") == NULL);
    CHECK(!dp_needs_su_bootstrap(&t, &c));
    CHECK(dp_secret_assign_cstr(&c.bootstrap_root_password, "root-password") == NULL);
    CHECK(dp_needs_su_bootstrap(&t, &c));
    /* Stock Alpine's doas can look usable to the probe and then refuse for
     * want of a TTY; a supplied root secret makes su authoritative. */
    t.has_doas = true;
    CHECK(dp_needs_su_bootstrap(&t, &c));
    omt_strlcpy(t.uid, "0", sizeof(t.uid));
    CHECK(!dp_needs_su_bootstrap(&t, &c));
    dp_connection_free(&c);
}

static size_t find(const char *haystack, const char *needle) {
    const char *at = strstr(haystack, needle);
    CHECK_MSG(at != NULL, "missing: %s", needle);
    return at ? (size_t)(at - haystack) : 0;
}

static size_t count(const char *haystack, const char *needle) {
    size_t n = 0;
    for (const char *p = strstr(haystack, needle); p; p = strstr(p + 1, needle)) n++;
    return n;
}

static void wifi_discovers_the_wireless_interface_instead_of_hardcoding_wlan0(void) {
    find(DP_WIFI_SCRIPT, "iface=${path#/sys/class/net/}");
    find(DP_WIFI_SCRIPT, "wpa_cli -i \"$iface\" ping");
    find(DP_WIFI_SCRIPT, "iw dev \"$iface\" set power_save off");
    find(DP_WIFI_SCRIPT, "set_network \"$network_id\" freq_list");
    CHECK(strstr(DP_WIFI_SCRIPT, "wpa_cli -i wlan0") == NULL);
}

static void wifi_profile_replacement_is_verified_or_explicitly_deferred(void) {
    size_t disconnect = find(DP_WIFI_SCRIPT, "wpa_cli -i \"$iface\" disconnect");
    size_t enable = find(DP_WIFI_SCRIPT, "wpa_cli -i \"$iface\" enable_network");
    size_t completed = find(DP_WIFI_SCRIPT, "status_state\" = COMPLETED");
    size_t removal = find(DP_WIFI_SCRIPT, "wpa_cli -i \"$iface\" remove_network");
    CHECK(disconnect < enable);
    CHECK(completed < removal);
    find(DP_WIFI_SCRIPT, "if [ \"$preserve\" = no ] && [ \"$activate\" = yes ]; then");
    find(DP_WIFI_SCRIPT, "New Wi-Fi profile did not associate; existing profiles were retained");
    find(DP_WIFI_SCRIPT, "wpa_cli -i \"$iface\" reconfigure");
    CHECK_INT(count(DP_WIFI_SCRIPT, "secure_saved_config\n"), 3);
    find(DP_WIFI_SCRIPT, "chmod 600 \"$config_path\"");
    find(DP_WIFI_SCRIPT, "chown root:root \"$config_path\"");
}

static void wifi_defaults_the_regulatory_country_to_us_before_scanning(void) {
    size_t country = find(DP_WIFI_SCRIPT, "set country US");
    size_t scan = find(DP_WIFI_SCRIPT, "wpa_cli -i \"$iface\" scan");
    CHECK(country < scan);
    find(DP_WIFI_SCRIPT, "get country");
    find(DP_WIFI_SCRIPT, "[A-Z][A-Z]) ;;");
}

static void wifi_update_script_is_valid_posix_shell(void) {
    write_text(scratch_path("wifi.sh"), DP_WIFI_SCRIPT);
    const char *args[] = {"-n", scratch_path("wifi.sh"), NULL};
    dp_process_result r;
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_run_process("/bin/sh", args, "/tmp", NULL, NULL, &r, &err));
    CHECK_MSG(r.exit_code == 0, "%s", omt_buf_cstr(&r.output));
    dp_process_result_free(&r);
    dp_err_free(&err);
}

static void first_web_password_reads_the_entrypoint_banner(void) {
    const char *logs = "============================================\n"
                       " Web UI password (save this now):\n"
                       " hunter2-web\n"
                       "============================================\n"
                       "omt-web listening on https://0.0.0.0:5000\n";
    omt_buf out;
    omt_buf_init(&out, 256);
    CHECK(dp_first_web_password(logs, &out));
    CHECK_STR(omt_buf_cstr(&out), "hunter2-web");
    omt_buf_clear(&out);
    CHECK(!dp_first_web_password("no password here", &out));
    CHECK(!dp_first_web_password(" Web UI password (save this now):\n=====\n", &out));
    omt_buf_free(&out);
}

static void post_install_reboot_is_the_same_fixed_action_as_manage(void) {
    dp_connection c;
    connection(&c, "pi");
    CHECK(dp_secret_assign_cstr(&c.sudo_password, "hunter2") == NULL);
    omt_buf a, joined, b;
    omt_buf_init(&a, 1024);
    omt_buf_init(&joined, 1024);
    omt_buf_init(&b, 1024);
    const char *const *argv = dp_action_argv(DP_ACTION_REBOOT);
    dp_privileged_argv_command(&c, argv, &a);
    for (size_t i = 0; argv[i]; i++) {
        if (i) omt_buf_putc(&joined, ' ');
        dp_shell_quote(&joined, argv[i]);
    }
    dp_privileged_command(&c, omt_buf_cstr(&joined), &b);
    CHECK_STR(omt_buf_cstr(&a), omt_buf_cstr(&b));
    omt_buf_free(&a);
    omt_buf_free(&joined);
    omt_buf_free(&b);
    dp_connection_free(&c);
}

static void alpine_sys_setup_uses_the_fixed_script_and_marker(void) {
    CHECK_STR(DP_SETUP_SYS_MEMBER, "deploy/host/setup-sys.sh");
    CHECK(strstr(DP_SETUP_SYS_COMPLETE, "Alpine sys install complete") != NULL);
}

static void require_success_quotes_the_remote_output(void) {
    ssh_result r;
    ssh_result_init(&r);
    dp_err err;
    dp_err_init(&err);
    r.exit_code = 0;
    CHECK(dp_require_success(&r, "Probe", &err));
    r.exit_code = 3;
    CHECK(!dp_require_success(&r, "Probe", &err));
    CHECK_STR(dp_err_text(&err), "Probe failed");
    omt_buf_puts(&r.out, "partial");
    omt_buf_puts(&r.err, "denied\n");
    CHECK(!dp_require_success(&r, "Probe", &err));
    CHECK_STR(dp_err_text(&err), "Probe failed:\npartial\ndenied\n");
    ssh_result_free(&r);
    dp_err_free(&err);
}

/* ------------------------------------------------------------------ ssh.rs */

static void an_explicit_known_hosts_path_overrides_the_home_default(void) {
    dp_connection c;
    connection(&c, "pi");
    c.known_hosts_path = dp_strdup("/trusted/known_hosts");
    omt_buf path;
    omt_buf_init(&path, 1024);
    dp_err err;
    dp_err_init(&err);
    CHECK(ssh_known_hosts_path(&c, &path, &err));
    CHECK_STR(omt_buf_cstr(&path), "/trusted/known_hosts");
    free(c.known_hosts_path);
    c.known_hosts_path = NULL;
    omt_buf_clear(&path);
    setenv("HOME", "/home/operator", 1);
    CHECK(ssh_known_hosts_path(&c, &path, &err));
    CHECK_STR(omt_buf_cstr(&path), "/home/operator/.ssh/known_hosts");
    omt_buf_free(&path);
    dp_err_free(&err);
    dp_connection_free(&c);
}

static void shell_upload_quotes_the_remote_path(void) {
    omt_buf b;
    omt_buf_init(&b, 1024);
    ssh_shell_upload_command(&b, "/tmp/omt-setup-sys-abcd.sh");
    CHECK_STR(omt_buf_cstr(&b), "umask 077 && cat > '/tmp/omt-setup-sys-abcd.sh'");
    omt_buf_clear(&b);
    ssh_shell_upload_command(&b, "/tmp/o'mt.sh");
    CHECK_STR(omt_buf_cstr(&b), "umask 077 && cat > '/tmp/o'\\''mt.sh'");
    omt_buf_free(&b);
}

static void readiness_markers_are_found_only_as_complete_byte_sequences(void) {
    const char hay[] = "before\nready\r\nafter";
    CHECK(ssh_contains_bytes((const uint8_t *)hay, sizeof(hay) - 1, "ready"));
    CHECK(!ssh_contains_bytes((const uint8_t *)"rea", 3, "ready"));
    CHECK(!ssh_contains_bytes((const uint8_t *)"anything", 8, ""));
}

static void combined_output_separates_the_streams_with_one_newline(void) {
    ssh_result r;
    ssh_result_init(&r);
    omt_buf out;
    omt_buf_init(&out, 1024);
    omt_buf_puts(&r.out, "out");
    omt_buf_puts(&r.err, "err");
    ssh_result_combined(&r, &out);
    CHECK_STR(omt_buf_cstr(&out), "out\nerr");
    omt_buf_clear(&out);
    omt_buf_clear(&r.out);
    omt_buf_puts(&r.out, "line\n");
    ssh_result_combined(&r, &out);
    CHECK_STR(omt_buf_cstr(&out), "line\nerr");
    omt_buf_free(&out);
    ssh_result_free(&r);
}

/* ---------------------------------------------------------------- tools.rs */

static void windows_builds_through_bash_and_never_needs_make(void) {
    dp_build_plan plan;
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_plan_from(NULL, "C:\\Program Files\\Git\\bin\\bash.exe", true, &plan, &err));
    CHECK_STR(plan.program, "C:\\Program Files\\Git\\bin\\bash.exe");
    CHECK_STR(plan.args[0], "scripts/build-arm64.sh");
    CHECK(plan.args[1] == NULL);
    CHECK_STR(plan.env[0], "ARM64_TARBALL=omt-client-arm64.tar.gz");
    dp_build_plan_free(&plan);
    /* GNU Make on Windows hands the script to cmd.exe, so it is not a
     * substitute for the shell. */
    CHECK(!dp_plan_from("C:\\make.exe", NULL, true, &plan, &err));
    CHECK(strstr(dp_err_text(&err), "Git for Windows") != NULL);
    CHECK(strstr(dp_err_text(&err), "embedded in this deployer") != NULL);
    CHECK(dp_plan_from("C:\\make.exe", "C:\\Git\\bin\\bash.exe", true, &plan, &err));
    CHECK_STR(plan.program, "C:\\Git\\bin\\bash.exe");
    dp_build_plan_free(&plan);
    dp_err_free(&err);
}

static void unix_prefers_make_and_falls_back_to_the_script(void) {
    dp_build_plan plan;
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_plan_from("/usr/bin/make", "/bin/bash", false, &plan, &err));
    CHECK_STR(plan.program, "/usr/bin/make");
    CHECK_STR(plan.args[0], "build-arm64");
    CHECK_STR(plan.args[1], "ARM64_TARBALL=omt-client-arm64.tar.gz");
    CHECK(plan.env[0] == NULL);
    omt_buf summary;
    omt_buf_init(&summary, 1024);
    dp_build_plan_summary(&plan, &summary);
    CHECK_STR(omt_buf_cstr(&summary),
              "/usr/bin/make build-arm64 ARM64_TARBALL=omt-client-arm64.tar.gz");
    omt_buf_free(&summary);
    dp_build_plan_free(&plan);
    CHECK(dp_plan_from(NULL, "/bin/bash", false, &plan, &err));
    CHECK_STR(plan.args[0], "scripts/build-arm64.sh");
    CHECK_STR(plan.env[0], "ARM64_TARBALL=omt-client-arm64.tar.gz");
    dp_build_plan_free(&plan);
    CHECK(!dp_plan_from(NULL, NULL, false, &plan, &err));
    dp_err_free(&err);
}

static bool has_suffix(const dp_suffixes *s, const char *value) {
    for (size_t i = 0; i < s->count; i++) {
        if (strcmp(s->items[i], value) == 0) return true;
    }
    return false;
}

static void windows_executable_names_follow_pathext(void) {
    dp_suffixes s;
    dp_executable_suffixes(true, ".COM;.EXE;.BAT;.CMD;.VBS", &s);
    CHECK_STR(s.items[0], "");
    CHECK(has_suffix(&s, ".EXE"));
    CHECK(has_suffix(&s, ".CMD"));
    dp_suffixes_free(&s);
    dp_executable_suffixes(false, ".EXE", &s);
    CHECK_INT(s.count, 1);
    dp_suffixes_free(&s);
    const char *broken[] = {NULL, "", ";;", "exe", ". exe"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(broken); i++) {
        dp_executable_suffixes(true, broken[i], &s);
        CHECK(has_suffix(&s, ".EXE"));
        dp_suffixes_free(&s);
    }
    dp_executable_suffixes(true, ".EXE;.exe;.EXE", &s);
    CHECK_INT(s.count, 2);
    dp_suffixes_free(&s);
}

static void the_wsl_launcher_is_not_a_build_shell(void) {
    CHECK(dp_is_wsl_launcher("C:\\Windows\\System32\\bash.exe"));
    CHECK(dp_is_wsl_launcher("c:\\windows\\sysnative\\bash.exe"));
    CHECK(!dp_is_wsl_launcher("C:\\Program Files\\Git\\bin\\bash.exe"));
    CHECK(!dp_is_wsl_launcher("/bin/bash"));
}

static bool fake_env(void *ctx, const char *name, omt_buf *out) {
    int mode = *(int *)ctx;
    if (mode == 0 && strcmp(name, "ProgramFiles") == 0) {
        omt_buf_puts(out, "C:\\Program Files");
        return true;
    }
    if (mode == 0 && strcmp(name, "LOCALAPPDATA") == 0) {
        omt_buf_puts(out, "C:\\Users\\op\\AppData\\Local");
        return true;
    }
    if (mode != 1 && strcmp(name, "SystemDrive") == 0) {
        omt_buf_puts(out, "C:");
        return true;
    }
    return false;
}

static bool candidate(const dp_candidates *c, const char *value) {
    for (size_t i = 0; i < c->count; i++) {
        if (strcmp(c->items[i], value) == 0) return true;
    }
    return false;
}

static void git_for_windows_is_found_where_it_installs(void) {
    int mode = 0;
    dp_candidates c;
    dp_windows_bash_candidates(fake_env, &mode, &c);
    /* Joined with this host's separator, as the Rust test built them. */
    CHECK(candidate(&c, "C:\\Program Files/Git/bin/bash.exe"));
    CHECK(candidate(&c, "C:\\Users\\op\\AppData\\Local/Programs/Git/bin/bash.exe"));
    CHECK(candidate(&c, "C:\\/msys64/usr/bin/bash.exe"));
    CHECK(c.count > 4);
    dp_candidates none, drive_only;
    int no_env = 1, drive = 2;
    dp_windows_bash_candidates(fake_env, &no_env, &none);
    dp_windows_bash_candidates(fake_env, &drive, &drive_only);
    CHECK_INT(none.count, drive_only.count);
    for (size_t i = 0; i < none.count && i < drive_only.count; i++) {
        CHECK_STR(none.items[i], drive_only.items[i]);
    }
    dp_candidates_free(&c);
    dp_candidates_free(&none);
    dp_candidates_free(&drive_only);
}

static void winget_packages_come_from_unsatisfied_rows_only(void) {
    dp_prerequisite rows[4] = {
        {"row", "purpose", true, true, NULL, NULL, &DP_DOCKER_DESKTOP},
        {"row", "purpose", true, false, NULL, NULL, &DP_GIT_FOR_WINDOWS},
        {"row", "purpose", true, false, NULL, NULL, &DP_GIT_FOR_WINDOWS},
        {"row", "purpose", true, false, NULL, NULL, NULL},
    };
    dp_prerequisites set = {rows, 4};
    const dp_package *out[8];
    CHECK_INT(dp_missing_packages(&set, out, 8), 1);
    CHECK(out[0] == &DP_GIT_FOR_WINDOWS);
    dp_prerequisites empty = {NULL, 0};
    CHECK_INT(dp_missing_packages(&empty, out, 8), 0);
}

static void a_missing_required_row_blocks_and_an_optional_one_does_not(void) {
    dp_prerequisite row = {"row", "purpose", true, false, NULL, NULL, NULL};
    CHECK(dp_prerequisite_blocking(&row));
    row.satisfied = true;
    CHECK(!dp_prerequisite_blocking(&row));
    row.required = false;
    row.satisfied = false;
    CHECK(!dp_prerequisite_blocking(&row));
}

static void progress_counter(void *ctx, const char *line) {
    (void)line;
    (*(int *)ctx)++;
}

static void an_already_installed_package_is_not_an_installation_failure(void) {
    int lines = 0;
    dp_progress p = {progress_counter, &lines};
    dp_process_result r;
    omt_buf_init(&r.output, 64);
    dp_err err;
    dp_err_init(&err);
    r.exit_code = 0;
    CHECK(dp_report_install(&DP_GIT_FOR_WINDOWS, &r, &p, &err));
    r.exit_code = DP_WINGET_ALREADY_INSTALLED;
    CHECK(dp_report_install(&DP_GIT_FOR_WINDOWS, &r, &p, &err));
    r.exit_code = 1;
    CHECK(!dp_report_install(&DP_GIT_FOR_WINDOWS, &r, &p, &err));
    CHECK_INT(lines, 2);
    CHECK_INT(DP_WINGET_ALREADY_INSTALLED, -1978335189);
    dp_err_free(&err);
}

static bool row_named(const dp_prerequisites *rows, const char *name, const dp_prerequisite **out) {
    for (size_t i = 0; i < rows->count; i++) {
        if (strcmp(rows->rows[i].name, name) == 0) {
            *out = &rows->rows[i];
            return true;
        }
    }
    return false;
}

static void probing_this_workstation_reports_the_project_it_was_given(void) {
    dp_cancel cancel = false;
    dp_prerequisites rows;
    const dp_prerequisite *row;
    CHECK(dp_probe_prerequisites(".", &cancel, &rows));
    CHECK(row_named(&rows, "Project source tree", &row) && row->satisfied);
    CHECK(row_named(&rows, "Container engine", &row));
    CHECK(row_named(&rows, "POSIX shell", &row));
    dp_prerequisites_free(&rows);
    CHECK(dp_probe_prerequisites("/nonexistent-omt-project", &cancel, &rows));
    CHECK(row_named(&rows, "Project source tree", &row) && dp_prerequisite_blocking(row));
    dp_prerequisites_free(&rows);
}

/* The fixture capsule links into this suite, so the embedded report is
 * exercised without the real image. */
static void an_embedded_deployment_needs_nothing_from_this_workstation(void) {
    dp_cancel cancel = false;
    dp_prerequisites rows;
    const dp_prerequisite *row;
    CHECK(dp_probe_prerequisites(NULL, &cancel, &rows));
    CHECK(rows.count > 0);
    for (size_t i = 0; i < rows.count; i++) CHECK(rows.rows[i].satisfied);
    CHECK(!row_named(&rows, "Container engine", &row));
    CHECK(!row_named(&rows, "Project source tree", &row));
    const dp_package *out[8];
    CHECK_INT(dp_missing_packages(&rows, out, 8), 0);
    dp_prerequisites_free(&rows);
}

static void installation_is_refused_where_there_is_no_winget(void) {
    dp_cancel cancel = false;
    dp_err err;
    dp_err_init(&err);
    const dp_package *pkgs[] = {&DP_GIT_FOR_WINDOWS};
    CHECK(!dp_install_packages(pkgs, 1, &cancel, NULL, &err));
    CHECK(strstr(dp_err_text(&err), "make install") != NULL);
    dp_err_free(&err);
}

static void the_windows_remedy_is_never_a_linux_only_installer(void) {
    omt_buf out;
    omt_buf_init(&out, 4096);
    dp_emulation_failure("docker", true, true, "exec /bin/sh: exec format error", &out);
    CHECK(strstr(omt_buf_cstr(&out), "make setup-arm64-emulation") == NULL);
    CHECK(strstr(omt_buf_cstr(&out), "Windows containers") != NULL);
    CHECK(strstr(omt_buf_cstr(&out), "exec format error") != NULL);
    omt_buf_clear(&out);
    dp_emulation_failure("podman", false, false, "", &out);
    CHECK(strstr(omt_buf_cstr(&out), "make setup-arm64-emulation") != NULL);
    CHECK(out.data[out.len - 1] != '\n');
    omt_buf_free(&out);
}

static void the_probe_verdict_is_the_machine_name_the_container_printed(void) {
    dp_process_result r;
    omt_buf_init(&r.output, 4096);
    struct {
        int code;
        const char *text;
        bool expect;
    } cases[] = {
        {0, "aarch64\n", true},
        {0, "Unable to find image locally\nbookworm-slim: Pulling from library/debian\naarch64\r\n",
         true},
        {0, "x86_64\n", false},
        {1, "exec /bin/sh: exec format error", false},
        {125, "aarch64\n", false},
    };
    for (size_t i = 0; i < OMT_ARRAY_LEN(cases); i++) {
        omt_buf_clear(&r.output);
        omt_buf_puts(&r.output, cases[i].text);
        r.exit_code = cases[i].code;
        CHECK_MSG(dp_reports_aarch64(&r) == cases[i].expect, "case %zu", i);
    }
    omt_buf_free(&r.output);
}

static void a_failed_repair_is_reported_differently_from_an_unattempted_one(void) {
    omt_buf a, b;
    omt_buf_init(&a, 4096);
    omt_buf_init(&b, 4096);
    dp_emulation_failure("docker", true, true, "", &a);
    dp_emulation_failure("docker", true, false, "", &b);
    CHECK(strcmp(omt_buf_cstr(&a), omt_buf_cstr(&b)) != 0);
    CHECK(strstr(omt_buf_cstr(&a), "still") != NULL);
    omt_buf_free(&a);
    omt_buf_free(&b);
}

/* ----------------------------------------------------------------- jobs.rs */

static dp_connection_fields fields(void) {
    dp_connection_fields f = {"pi.local", "pi", "ssh-password", "", "", ""};
    return f;
}

static void the_three_credentials_stay_separate(void) {
    dp_connection_fields f = fields();
    f.sudo_password = "sudo-password";
    f.bootstrap_root_password = "root-password";
    dp_connection c;
    CHECK(dp_connection_from_fields(&f, &c) == NULL);
    CHECK_STR(dp_secret_text(&c.password), "ssh-password");
    CHECK_STR(dp_secret_text(&c.sudo_password), "sudo-password");
    CHECK_STR(dp_secret_text(&c.bootstrap_root_password), "root-password");
    dp_connection_free(&c);
}

static void an_empty_ssh_password_survives_where_the_optional_ones_do_not(void) {
    dp_connection_fields f = fields();
    f.username = "root";
    f.password = "";
    dp_connection c;
    CHECK(dp_connection_from_fields(&f, &c) == NULL);
    CHECK(c.password.set);
    CHECK_STR(dp_secret_text(&c.password), "");
    CHECK(!c.sudo_password.set);
    CHECK(!c.bootstrap_root_password.set);
    CHECK(c.known_hosts_path == NULL);
    dp_connection_free(&c);
}

static void surrounding_whitespace_is_trimmed_from_the_typed_fields(void) {
    dp_connection_fields f = fields();
    f.host = "  pi.local\t";
    f.username = " pi ";
    dp_connection c;
    CHECK(dp_connection_from_fields(&f, &c) == NULL);
    CHECK_STR(c.host, "pi.local");
    CHECK_STR(c.username, "pi");
    dp_connection_free(&c);
}

static void an_invalid_host_is_refused_before_anything_connects(void) {
    dp_connection c;
    dp_connection_fields f = fields();
    f.host = " -pi.local ";
    CHECK(dp_connection_from_fields(&f, &c) != NULL);
    dp_connection_free(&c);
    f = fields();
    f.username = "ro ot";
    CHECK(dp_connection_from_fields(&f, &c) != NULL);
    dp_connection_free(&c);
    f = fields();
    f.known_hosts = "/nonexistent/known_hosts";
    CHECK(dp_connection_from_fields(&f, &c) != NULL);
    dp_connection_free(&c);
}

/* ------------------------------------------------------------- sd_card.rs */

static void sd(dp_sd_settings *s, const char *path) {
    s->boot_directory = dp_strdup(path);
    s->country = dp_strdup("US");
    s->wifi_ssid = dp_strdup("Studio Wi-Fi");
    secret(&s->wifi_password, "correct horse battery staple");
}

static void wifi_file_uses_linux_lines_and_derived_credentials(void) {
    dp_sd_settings s;
    sd(&s, "unused");
    omt_buf out;
    omt_buf_init(&out, 4096);
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_wpa_supplicant_config(&s, &out, &err));
    const char *text = omt_buf_cstr(&out);
    CHECK(omt_has_prefix(text, "country=US\nnetwork={\n"));
    CHECK(strchr(text, '\r') == NULL);
    CHECK(strstr(text, "Studio Wi-Fi") == NULL);
    CHECK(strstr(text, "correct horse battery staple") == NULL);
    CHECK(strstr(text, "    ssid=53747564696f2057692d4669\n") != NULL);
    const char *psk = strstr(text, "    psk=");
    CHECK(psk != NULL);
    if (psk) {
        psk += 8;
        size_t n = strcspn(psk, "\n");
        CHECK_INT(n, 64);
        for (size_t i = 0; i < n; i++) CHECK(strchr("0123456789abcdef", psk[i]) != NULL);
    }
    omt_buf_free(&out);
    dp_err_free(&err);
    dp_sd_settings_free(&s);
}

static void validation_requires_an_alpine_boot_partition_and_country(void) {
    char root[512];
    omt_snprintf(root, sizeof(root), "%s/sd-card", scratch);
    mkdir(root, 0700);
    dp_sd_settings s;
    sd(&s, root);
    dp_err err;
    dp_err_init(&err);
    const char *problem = dp_validate_sd_settings(&s, &err);
    CHECK(problem && strstr(problem, "not an Alpine boot partition"));
    char path[600];
    omt_snprintf(path, sizeof(path), "%s/.alpine-release", root);
    write_text(path, "3.24.2\n");
    omt_snprintf(path, sizeof(path), "%s/config.txt", root);
    write_text(path, "[all]\n");
    omt_snprintf(path, sizeof(path), "%s/boot", root);
    mkdir(path, 0700);
    CHECK(dp_validate_sd_settings(&s, &err) == NULL);
    free(s.country);
    s.country = dp_strdup("us");
    CHECK(dp_validate_sd_settings(&s, &err) != NULL);
    dp_err_free(&err);
    dp_sd_settings_free(&s);
}

/* ------------------------------------------------------------- the capsule */

static void the_fixture_capsule_is_linked_and_described(void) {
    size_t n = 0;
    const dp_capsule_member *members = dp_capsule_members(&n);
    CHECK_INT(n, 4);
    CHECK_STR(members[0].name, "omt-client-arm64.tar.gz");
    CHECK(dp_capsule_member_named("deploy/host/set-hostname.sh") != NULL);
    CHECK(dp_capsule_member_named("deploy/host/nothing.sh") == NULL);
    const dp_capsule_member *image = dp_capsule_image();
    CHECK(image && image->size > 2 && image->bytes[0] == 0x1f && image->bytes[1] == 0x8b);
    size_t len;
    CHECK_STR(dp_license_text(&len), "MIT License\n");
    CHECK_INT(len, 12);
    omt_buf report;
    omt_buf_init(&report, 1024);
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_capsule_report(&report, &err));
    CHECK(omt_has_prefix(omt_buf_cstr(&report), "Embedded capsule: 4 members, "
                                                "omt-client-arm64.tar.gz 0 MiB, sha256 "));
    omt_buf_free(&report);
    dp_err_free(&err);
}

int main(void) {
    omt_snprintf(scratch, sizeof(scratch), "/tmp/omt-deploy-test-%d", (int)getpid());
    mkdir(scratch, 0700);
    RUN(validation_contract);
    RUN(secrets_refuse_controls_and_oversize);
    RUN(restart_manages_the_service_even_before_first_container_creation);
    RUN(reboot_is_a_fixed_deferred_host_action);
    RUN(hex_is_lower_case_and_fixed_width);
    RUN(psk_vector);
    RUN(manifest_requires_transaction_members);
    RUN(manifest_names_are_safe);
    RUN(cancelled_process_is_not_spawned);
    RUN(process_captures_both_streams_and_nonzero_exit);
    RUN(cancellation_remains_live_after_the_direct_child_exits);
    RUN(output_limit_keeps_the_first_four_mib);
    RUN(accepts_every_supported_board);
    RUN(refuses_unsupported_boards_and_near_misses);
    RUN(still_refuses_the_wrong_architecture_or_distribution);
    RUN(picks_an_escalation_for_each_stock_alpine_shape);
    RUN(parses_the_tooling_probe);
    RUN(reports_the_detected_board_for_progress);
    RUN(digest_parser_accepts_sha256sum_output);
    RUN(redaction_avoids_infinite_loops_on_marker_substrings);
    RUN(successful_installs_surface_only_the_final_summary);
    RUN(sudo_input_is_newline_terminated_and_prefixes_follow_the_account);
    RUN(web_password_action_uses_only_a_fixed_stdin_command);
    RUN(the_rename_command_names_only_the_uploaded_script);
    RUN(untouched_alpine_can_bootstrap_through_su_only_with_a_root_secret);
    RUN(wifi_discovers_the_wireless_interface_instead_of_hardcoding_wlan0);
    RUN(wifi_profile_replacement_is_verified_or_explicitly_deferred);
    RUN(wifi_defaults_the_regulatory_country_to_us_before_scanning);
    RUN(wifi_update_script_is_valid_posix_shell);
    RUN(first_web_password_reads_the_entrypoint_banner);
    RUN(post_install_reboot_is_the_same_fixed_action_as_manage);
    RUN(alpine_sys_setup_uses_the_fixed_script_and_marker);
    RUN(require_success_quotes_the_remote_output);
    RUN(an_explicit_known_hosts_path_overrides_the_home_default);
    RUN(shell_upload_quotes_the_remote_path);
    RUN(readiness_markers_are_found_only_as_complete_byte_sequences);
    RUN(combined_output_separates_the_streams_with_one_newline);
    RUN(windows_builds_through_bash_and_never_needs_make);
    RUN(unix_prefers_make_and_falls_back_to_the_script);
    RUN(windows_executable_names_follow_pathext);
    RUN(the_wsl_launcher_is_not_a_build_shell);
    RUN(git_for_windows_is_found_where_it_installs);
    RUN(winget_packages_come_from_unsatisfied_rows_only);
    RUN(a_missing_required_row_blocks_and_an_optional_one_does_not);
    RUN(an_already_installed_package_is_not_an_installation_failure);
    RUN(probing_this_workstation_reports_the_project_it_was_given);
    RUN(an_embedded_deployment_needs_nothing_from_this_workstation);
    RUN(installation_is_refused_where_there_is_no_winget);
    RUN(the_windows_remedy_is_never_a_linux_only_installer);
    RUN(the_probe_verdict_is_the_machine_name_the_container_printed);
    RUN(a_failed_repair_is_reported_differently_from_an_unattempted_one);
    RUN(the_three_credentials_stay_separate);
    RUN(an_empty_ssh_password_survives_where_the_optional_ones_do_not);
    RUN(surrounding_whitespace_is_trimmed_from_the_typed_fields);
    RUN(an_invalid_host_is_refused_before_anything_connects);
    RUN(wifi_file_uses_linux_lines_and_derived_credentials);
    RUN(validation_requires_an_alpine_boot_partition_and_country);
    RUN(the_fixture_capsule_is_linked_and_described);
    const char *rm[] = {"-rf", scratch, NULL};
    dp_process_result r;
    dp_err err;
    dp_err_init(&err);
    if (dp_run_process("/bin/rm", rm, "/", NULL, NULL, &r, &err)) dp_process_result_free(&r);
    dp_err_free(&err);
    return TEST_EXIT();
}
