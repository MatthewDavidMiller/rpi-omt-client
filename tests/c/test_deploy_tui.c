/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The terminal deployer, ported from rpi-omt-deploy-tui's tests (app.rs,
 * main.rs, ui.rs), plus the VT input decoder that replaced crossterm.
 */
#include <sys/stat.h>
#include <unistd.h>

#include "deploy/tui/app.h"
#include "deploy/tui/screen.h"
#include "deploy/tui/ui.h"
#include "test.h"

static void set(app *a, field f, const char *text) {
    omt_buf_clear(&a->fields[f]);
    omt_buf_puts(&a->fields[f], text);
}

static const char *value(const app *a, field f) { return omt_buf_cstr(&a->fields[f]); }

static void with_log(app *a, size_t lines) {
    app_init(a);
    char text[32];
    for (size_t i = 0; i < lines; i++) {
        omt_snprintf(text, sizeof(text), "%zu", i);
        app_push_log(a, text);
    }
}

/* What a terminal would show, as text. */
static void render(const app *a, int w, int h, omt_buf *out) {
    screen s;
    screen_init(&s);
    CHECK(screen_begin(&s, w, h));
    ui_draw(&s, a);
    for (int y = 0; y < h; y++) {
        screen_row_text(&s, y, out);
        omt_buf_putc(out, '\n');
    }
    screen_free(&s);
}

/* ---------------------------------------------------------------- main.rs */

static void paging_up_leaves_the_tail_rather_than_the_beginning(void) {
    app a;
    with_log(&a, 500);
    CHECK(a.follow_log);
    app_scroll_log(&a, -1);
    CHECK(!a.follow_log);
    CHECK_INT(a.log_scroll, 489);
    app_free(&a);
}

static void paging_back_down_resumes_following(void) {
    app a;
    with_log(&a, 500);
    app_scroll_log(&a, -1);
    app_scroll_log(&a, 1);
    CHECK_INT(a.log_scroll, 499);
    app_scroll_log(&a, 1);
    CHECK(a.follow_log);
    app_free(&a);
}

static void paging_up_stops_at_the_start_of_the_log(void) {
    app a;
    with_log(&a, 30);
    for (int i = 0; i < 10; i++) app_scroll_log(&a, -1);
    CHECK_INT(a.log_scroll, 0);
    CHECK(!a.follow_log);
    app_free(&a);
}

static void paging_an_empty_log_is_harmless(void) {
    app a;
    with_log(&a, 0);
    app_scroll_log(&a, -1);
    CHECK_INT(a.log_scroll, 0);
    app_free(&a);
}

static void the_log_keeps_its_capacity(void) {
    app a;
    with_log(&a, LOG_CAPACITY + 5);
    CHECK_INT(a.log_len, LOG_CAPACITY);
    CHECK_STR(a.log[0], "5");
    app_free(&a);
}

/* ----------------------------------------------------------------- app.rs */

static void a_prefilled_first_field_is_edited_from_its_end(void) {
    app a;
    app_init(&a);
    for (size_t i = 0; i < strlen("raspberrypi.local"); i++) app_backspace(&a);
    CHECK_STR(value(&a, FIELD_HOST), "");
    set(&a, FIELD_HOST, "pi.local");
    app_select_view(&a, VIEW_DEPLOY);
    app_select_view(&a, VIEW_CONNECTION);
    app_backspace(&a);
    CHECK_STR(value(&a, FIELD_HOST), "pi.loca");
    app_free(&a);
}

static void editing_counts_characters_not_bytes(void) {
    app a;
    app_init(&a);
    app_select_view(&a, VIEW_WIFI);
    key_event k = {KEY_CHAR, 0x00E9, 0, false}; /* é */
    app_handle_key(&a, &k);
    k.ch = 'x';
    app_handle_key(&a, &k);
    CHECK_STR(value(&a, FIELD_WIFI_SSID), "\xc3\xa9x");
    app_move_cursor(&a, -1);
    app_backspace(&a);
    CHECK_STR(value(&a, FIELD_WIFI_SSID), "x");
    app_cursor_home(&a);
    app_delete(&a);
    CHECK_STR(value(&a, FIELD_WIFI_SSID), "");
    /* Control characters never reach a value. */
    k.ch = 0x07;
    app_handle_key(&a, &k);
    CHECK_STR(value(&a, FIELD_WIFI_SSID), "");
    app_free(&a);
}

static char scratch[256];

static void write_text(const char *path, const char *text) {
    FILE *f = fopen(path, "wb");
    if (!f) exit(2);
    fputs(text, f);
    fclose(f);
}

static void sd_card_job_is_local_and_uses_the_shared_validation(void) {
    char root[512], path[600];
    omt_snprintf(root, sizeof(root), "%s/sd", scratch);
    mkdir(root, 0700);
    omt_snprintf(path, sizeof(path), "%s/.alpine-release", root);
    write_text(path, "alpine-rpi-3.24.2\n");
    omt_snprintf(path, sizeof(path), "%s/config.txt", root);
    write_text(path, "[all]\n");
    omt_snprintf(path, sizeof(path), "%s/boot", root);
    mkdir(path, 0700);
    app a;
    app_init(&a);
    set(&a, FIELD_BOOT_DIRECTORY, root);
    set(&a, FIELD_WIFI_SSID, "studio");
    set(&a, FIELD_WIFI_PASSWORD, "passphrase");
    dp_err err;
    dp_err_init(&err);
    CHECK(app_precheck(&a, DP_JOB_PREPARE_SD, &err) == NULL);
    dp_job_request r;
    CHECK(app_request(&a, DP_JOB_PREPARE_SD, DP_ACTION_STATUS, &r) == NULL);
    CHECK(!r.has_connection);
    dp_job_request_free(&r);
    set(&a, FIELD_WIFI_COUNTRY, "us");
    CHECK(app_precheck(&a, DP_JOB_PREPARE_SD, &err) != NULL);
    dp_err_free(&err);
    app_free(&a);
}

static void factory_alpine_keeps_an_explicit_empty_ssh_password(void) {
    app a;
    app_init(&a);
    set(&a, FIELD_HOST, "10.1.20.223");
    set(&a, FIELD_USER, "root");
    set(&a, FIELD_PASSWORD, "");
    dp_job_request r;
    CHECK(app_request(&a, DP_JOB_TEST, DP_ACTION_STATUS, &r) == NULL);
    CHECK(r.has_connection);
    CHECK(r.connection.password.set);
    CHECK_STR(dp_secret_text(&r.connection.password), "");
    dp_job_request_free(&r);
    app_free(&a);
}

static void the_alpine_install_is_confirmed_before_anything_starts(void) {
    app a;
    app_init(&a);
    app_press(&a, SLOT_ALPINE_BUTTON);
    CHECK(a.confirm.active);
    CHECK(strstr(a.confirm.prompt, "Erase the boot disk") != NULL);
    CHECK(!a.confirm.quit && a.confirm.job == DP_JOB_ALPINE);
    CHECK(!app_busy(&a));
    app_confirm_pending(&a, false);
    CHECK(!a.confirm.active);
    CHECK(!app_busy(&a));
    CHECK_STR(omt_buf_cstr(&a.status), "Cancelled.");
    app_free(&a);
}

static void reading_status_is_not_confirmed(void) {
    app a;
    app_init(&a);
    /* Status starts a job at once; with no Pi it fails, but it never asks. */
    app_press(&a, SLOT_STATUS_BUTTON);
    CHECK(!a.confirm.active);
    if (a.job) {
        while (app_busy(&a)) {
            app_poll_worker(&a);
            dp_sleep_ms(20);
        }
    }
    app_free(&a);
}

static void restart_and_reboot_ask_first(void) {
    app a;
    app_init(&a);
    app_press(&a, SLOT_RESTART_BUTTON);
    CHECK(a.confirm.active && a.confirm.action == DP_ACTION_RESTART);
    app_confirm_pending(&a, false);
    app_press(&a, SLOT_REBOOT_BUTTON);
    CHECK(a.confirm.active && a.confirm.action == DP_ACTION_REBOOT);
    /* While a confirmation is open, other keys do nothing. */
    key_event tab = {KEY_TAB, 0, 0, false};
    size_t focus = a.focus;
    app_handle_key(&a, &tab);
    CHECK_INT(a.focus, focus);
    key_event esc = {KEY_ESC, 0, 0, false};
    app_handle_key(&a, &esc);
    CHECK(!a.confirm.active);
    app_free(&a);
}

static void quitting_asks_only_while_a_job_is_running(void) {
    app a;
    app_init(&a);
    app_request_quit(&a);
    CHECK(a.should_quit);
    app_free(&a);

    app busy;
    app_init(&busy);
    busy.job = worker_running_for_test();
    app_request_quit(&busy);
    CHECK(!busy.should_quit);
    CHECK(busy.confirm.active);
    app_confirm_pending(&busy, true);
    CHECK(busy.should_quit);
    app_free(&busy);
}

static void a_short_host_password_is_refused_by_the_precheck(void) {
    app a;
    app_init(&a);
    set(&a, FIELD_OS_ROOT_PASSWORD, "rootpass1");
    set(&a, FIELD_OS_ROOT_CONFIRM, "rootpass1");
    set(&a, FIELD_OS_PI_PASSWORD, "pipassword");
    set(&a, FIELD_OS_PI_CONFIRM, "pipassword");
    dp_err err;
    dp_err_init(&err);
    CHECK(app_precheck(&a, DP_JOB_ALPINE, &err) == NULL);
    set(&a, FIELD_OS_PI_PASSWORD, "short");
    set(&a, FIELD_OS_PI_CONFIRM, "short");
    CHECK(app_precheck(&a, DP_JOB_ALPINE, &err) != NULL);
    set(&a, FIELD_OS_PI_PASSWORD, "pipassword");
    set(&a, FIELD_OS_PI_CONFIRM, "pipassword2");
    CHECK_STR(app_precheck(&a, DP_JOB_ALPINE, &err), "pi password confirmation does not match");
    set(&a, FIELD_OS_PI_CONFIRM, "pipassword");
    set(&a, FIELD_WIFI_SSID, "studio");
    CHECK_STR(app_precheck(&a, DP_JOB_ALPINE, &err), "A Wi-Fi SSID needs a Wi-Fi password");
    dp_err_free(&err);
    app_free(&a);
}

static void deploy_refuses_a_remote_directory_the_core_would_reject(void) {
    app a;
    app_init(&a);
    dp_err err;
    dp_err_init(&err);
    CHECK(app_precheck(&a, DP_JOB_DEPLOY, &err) == NULL);
    const char *rejected[] = {"", "opt/omt-client", "/opt/omt-client/", "/opt/../etc"};
    for (size_t i = 0; i < 4; i++) {
        set(&a, FIELD_REMOTE_DIRECTORY, rejected[i]);
        CHECK_MSG(app_precheck(&a, DP_JOB_DEPLOY, &err) != NULL, "accepted %s", rejected[i]);
    }
    dp_err_free(&err);
    app_free(&a);
}

static void successful_alpine_setup_can_apply_the_new_pi_login(void) {
    app a;
    app_init(&a);
    set(&a, FIELD_USER, "root");
    set(&a, FIELD_OS_PI_PASSWORD, "new-pi-password");
    a.switch_to_pi_after_job = true;
    a.job = worker_finished_for_test(true, NULL);
    app_poll_worker(&a);
    CHECK_STR(value(&a, FIELD_USER), "pi");
    CHECK_STR(value(&a, FIELD_PASSWORD), "new-pi-password");
    CHECK_STR(value(&a, FIELD_SUDO_PASSWORD), "new-pi-password");
    CHECK(!a.switch_to_pi_after_job);
    CHECK(!app_busy(&a));
    app_free(&a);
}

static void a_failed_job_is_reported_in_the_status_and_the_log(void) {
    app a;
    app_init(&a);
    a.job = worker_finished_for_test(false, "it broke");
    app_poll_worker(&a);
    CHECK_STR(omt_buf_cstr(&a.status), "Failed: it broke");
    CHECK(a.log_len == 1 && strcmp(a.log[0], "-- failed: it broke --") == 0);
    app_free(&a);
}

static void web_password_rotation_uses_its_own_fields(void) {
    app a;
    app_init(&a);
    set(&a, FIELD_PASSWORD, "pw");
    set(&a, FIELD_WEB_PASSWORD, "deploy-view-password");
    set(&a, FIELD_MANAGE_WEB_PASSWORD, "manage-view-password");
    dp_job_request r;
    CHECK(app_request(&a, DP_JOB_WEB_PASSWORD, DP_ACTION_STATUS, &r) == NULL);
    CHECK_STR(dp_secret_text(&r.web_password), "manage-view-password");
    dp_job_request_free(&r);
    CHECK(app_request(&a, DP_JOB_DEPLOY, DP_ACTION_STATUS, &r) == NULL);
    CHECK_STR(dp_secret_text(&r.web_password), "deploy-view-password");
    dp_job_request_free(&r);
    app_free(&a);
}

static void keys_switch_views_and_reveal_secrets(void) {
    app a;
    app_init(&a);
    key_event f5 = {KEY_F, 0, 5, false};
    app_handle_key(&a, &f5);
    CHECK_INT(a.current, VIEW_MANAGE);
    key_event next = {KEY_RIGHT, 0, 0, true};
    app_handle_key(&a, &next);
    CHECK_INT(a.current, VIEW_WIFI);
    key_event prev = {KEY_LEFT, 0, 0, true};
    app_handle_key(&a, &prev);
    app_handle_key(&a, &prev);
    CHECK_INT(a.current, VIEW_DEPLOY);
    app_select_view(&a, VIEW_CONNECTION);
    app_handle_key(&a, &prev);
    CHECK_INT(a.current, VIEW_ABOUT);
    key_event reveal = {KEY_CHAR, 'r', 0, true};
    app_handle_key(&a, &reveal);
    CHECK(a.reveal);
    key_event quit = {KEY_CHAR, 'c', 0, true};
    app_handle_key(&a, &quit);
    CHECK(a.should_quit);
    app_free(&a);
}

/* ------------------------------------------------------------------ ui.rs */

static void about_reproduces_the_licence_and_the_third_party_notices(void) {
    ui_lines lines = {0};
    ui_about_text(80, &lines);
    omt_buf joined;
    omt_buf_init(&joined, 1u << 24);
    for (size_t i = 0; i < lines.count; i++) {
        omt_buf_puts(&joined, lines.items[i]);
        omt_buf_putc(&joined, '\n');
    }
    CHECK(strstr(omt_buf_cstr(&joined), OMT_VERSION) != NULL);
    CHECK(strstr(omt_buf_cstr(&joined), "LICENSE\n-------") != NULL);
    CHECK(strstr(omt_buf_cstr(&joined), "THIRD-PARTY NOTICES") != NULL);
    /* Every line of both texts is reproduced. */
    size_t len;
    const char *texts[2] = {dp_license_text(&len), dp_notices_text(&len)};
    for (int t = 0; t < 2; t++) {
        const char *p = texts[t];
        while (*p) {
            const char *nl = strchr(p, '\n');
            size_t n = nl ? (size_t)(nl - p) : strlen(p);
            while (n > 0 && (p[n - 1] == ' ' || p[n - 1] == '\r')) n--;
            char *line = dp_strndup(p, n);
            CHECK_MSG(line && strstr(omt_buf_cstr(&joined), line), "missing: %s", line ? line : "");
            free(line);
            if (!nl) break;
            p = nl + 1;
        }
    }
    omt_buf_free(&joined);
    ui_lines_free(&lines);
}

static void wrapping_keeps_blank_lines_and_indentation(void) {
    ui_lines lines = {0};
    ui_wrap("head\n\n  a bb ccc", 6, &lines);
    CHECK_INT(lines.count, 4);
    if (lines.count == 4) {
        CHECK_STR(lines.items[0], "head");
        CHECK_STR(lines.items[1], "");
        CHECK_STR(lines.items[2], "  a bb");
        CHECK_STR(lines.items[3], "  ccc");
    }
    ui_lines_free(&lines);
}

static void a_word_wider_than_the_view_is_broken_rather_than_clipped(void) {
    ui_lines lines = {0};
    ui_wrap("abcdefgh", 3, &lines);
    CHECK_INT(lines.count, 3);
    if (lines.count == 3) {
        CHECK_STR(lines.items[0], "abc");
        CHECK_STR(lines.items[1], "def");
        CHECK_STR(lines.items[2], "gh");
    }
    ui_lines_free(&lines);
}

static void the_document_fits_the_width_it_wrapped_to(void) {
    const size_t widths[] = {38, 60, 120};
    for (size_t w = 0; w < 3; w++) {
        ui_lines lines = {0};
        ui_about_text(widths[w], &lines);
        for (size_t i = 0; i < lines.count; i++) {
            CHECK_MSG(dp_utf8_count(lines.items[i], strlen(lines.items[i])) <= widths[w], "%zu: %s",
                      widths[w], lines.items[i]);
        }
        ui_lines_free(&lines);
    }
}

static void the_end_of_the_notices_can_be_scrolled_to(void) {
    app a;
    app_init(&a);
    a.current = VIEW_ABOUT;
    a.about_scroll = SIZE_MAX;
    omt_buf out;
    omt_buf_init(&out, 1u << 20);
    render(&a, 80, 30, &out);
    size_t len;
    const char *notices = dp_notices_text(&len);
    while (len > 0 && (notices[len - 1] == '\n' || notices[len - 1] == ' ')) len--;
    const char *tail = notices + len;
    while (tail > notices && tail[-1] != '\n') tail--;
    char *last = dp_strndup(tail, (size_t)(notices + len - tail));
    CHECK_MSG(last && strstr(omt_buf_cstr(&out), last), "%s", omt_buf_cstr(&out));
    free(last);
    a.about_scroll = 0;
    omt_buf_clear(&out);
    render(&a, 80, 30, &out);
    CHECK(strstr(omt_buf_cstr(&out), "Raspberry Pi OMT client deployer") != NULL);
    omt_buf_free(&out);
    app_free(&a);
}

static void a_short_terminal_scrolls_the_form_to_the_focused_row(void) {
    app a;
    app_init(&a);
    app_select_view(&a, VIEW_ALPINE);
    size_t slots;
    view_slots(VIEW_ALPINE, &slots);
    CHECK(slots > 5);
    omt_buf out;
    omt_buf_init(&out, 1u << 20);
    /* 14 rows: 3 header, 3 status, 2 borders, 6 for nine fields. */
    render(&a, 80, 14, &out);
    CHECK(strstr(omt_buf_cstr(&out), "Appliance hostname") != NULL);
    CHECK(strstr(omt_buf_cstr(&out), "Run Alpine setup") == NULL);
    a.focus = slots - 1;
    omt_buf_clear(&out);
    render(&a, 80, 14, &out);
    CHECK(strstr(omt_buf_cstr(&out), "Run Alpine setup") != NULL);
    CHECK(strstr(omt_buf_cstr(&out), "fields") != NULL);
    omt_buf_free(&out);
    app_free(&a);
}

static void a_tall_terminal_shows_every_row_at_once(void) {
    app a;
    app_init(&a);
    app_select_view(&a, VIEW_ALPINE);
    omt_buf out;
    omt_buf_init(&out, 1u << 20);
    render(&a, 80, 30, &out);
    CHECK(strstr(omt_buf_cstr(&out), "Appliance hostname") != NULL);
    CHECK(strstr(omt_buf_cstr(&out), "Run Alpine setup") != NULL);
    CHECK(strstr(omt_buf_cstr(&out), "fields 1-") == NULL);
    omt_buf_free(&out);
    app_free(&a);
}

static void a_narrow_terminal_shows_the_end_of_the_value_being_typed(void) {
    app a;
    app_init(&a);
    app_select_view(&a, VIEW_SD_CARD);
    a.focus = 0;
    const char *path = "/run/media/operator/ALPINE-BOOT-PARTITION";
    set(&a, FIELD_BOOT_DIRECTORY, path);
    a.cursor = strlen(path);
    omt_buf out;
    omt_buf_init(&out, 1u << 20);
    render(&a, 40, 20, &out);
    CHECK(strstr(omt_buf_cstr(&out), "PARTITION") != NULL);
    omt_buf_free(&out);
    app_free(&a);
}

static void the_label_gutter_leaves_room_for_a_value(void) {
    const int widths[] = {38, 40, 60, 80, 200};
    for (size_t i = 0; i < 5; i++) {
        size_t gutter = ui_label_width(widths[i]);
        CHECK(gutter >= 12 && gutter <= 32);
        CHECK(gutter < (size_t)widths[i]);
    }
    omt_buf out;
    omt_buf_init(&out, 64);
    ui_truncate("short", 10, &out);
    CHECK_STR(omt_buf_cstr(&out), "short");
    omt_buf_clear(&out);
    ui_truncate("far too long to fit", 10, &out);
    CHECK_STR(omt_buf_cstr(&out), "far too l~");
    omt_buf_free(&out);
}

static void the_visible_window_always_contains_the_focus(void) {
    for (size_t count = 1; count < 12; count++) {
        for (size_t visible = 1; visible <= count; visible++) {
            for (size_t focus = 0; focus < count; focus++) {
                size_t first = ui_first_visible_slot(focus, count, visible);
                CHECK(first + visible <= count);
                CHECK(focus >= first && focus < first + visible);
            }
        }
    }
}

static void secrets_are_masked_until_revealed(void) {
    app a;
    app_init(&a);
    set(&a, FIELD_PASSWORD, "hunter2");
    omt_buf out;
    omt_buf_init(&out, 1u << 20);
    render(&a, 80, 24, &out);
    CHECK(strstr(omt_buf_cstr(&out), "hunter2") == NULL);
    CHECK(strstr(omt_buf_cstr(&out), "*******") != NULL);
    a.reveal = true;
    omt_buf_clear(&out);
    render(&a, 80, 24, &out);
    CHECK(strstr(omt_buf_cstr(&out), "hunter2") != NULL);
    omt_buf_free(&out);
    app_free(&a);
}

static void too_small_a_terminal_says_so(void) {
    app a;
    app_init(&a);
    omt_buf out;
    omt_buf_init(&out, 4096);
    render(&a, 30, 8, &out);
    CHECK(strstr(omt_buf_cstr(&out), "Terminal is 30x8") != NULL);
    omt_buf_free(&out);
    app_free(&a);
}

/* ---------------------------------------------------------------- input */

static int decode(const char *bytes, bool final, key_event *out) {
    int count = 0;
    term_decode((const uint8_t *)bytes, strlen(bytes), final, out, 16, &count);
    return count;
}

static void vt_input_decodes_to_the_keys_the_app_handles(void) {
    key_event k[16];
    CHECK_INT(decode("\x1b[A\x1b[B\x1b[C\x1b[D", false, k), 4);
    CHECK(k[0].code == KEY_UP && k[3].code == KEY_LEFT);
    CHECK_INT(decode("\x1b[1;5C\x1b[1;5D", false, k), 2);
    CHECK(k[0].code == KEY_RIGHT && k[0].ctrl && k[1].code == KEY_LEFT && k[1].ctrl);
    CHECK_INT(decode("\x1bOP\x1b[15~\x1b[19~", false, k), 3);
    CHECK(k[0].code == KEY_F && k[0].f == 1 && k[1].f == 5 && k[2].f == 8);
    CHECK_INT(decode("\x1b[H\x1b[F\x1b[3~\x1b[5~\x1b[6~\x1b[Z", false, k), 6);
    CHECK(k[0].code == KEY_HOME && k[1].code == KEY_END && k[2].code == KEY_DELETE &&
          k[3].code == KEY_PAGE_UP && k[4].code == KEY_PAGE_DOWN && k[5].code == KEY_BACKTAB);
    CHECK_INT(decode("\r\t\x7f\x11\x12\x03", false, k), 6);
    CHECK(k[0].code == KEY_ENTER && k[1].code == KEY_TAB && k[2].code == KEY_BACKSPACE);
    CHECK(k[3].code == KEY_CHAR && k[3].ctrl && k[3].ch == 'q');
    CHECK(k[4].ch == 'r' && k[5].ch == 'c');
    CHECK_INT(decode("a\xc3\xa9\xe2\x82\xac", false, k), 3);
    CHECK(k[1].ch == 0xE9 && k[2].ch == 0x20AC);
    /* A lone Esc only once nothing more is coming. */
    CHECK_INT(decode("\x1b", false, k), 0);
    CHECK_INT(decode("\x1b", true, k), 1);
    CHECK(k[0].code == KEY_ESC);
    /* A split UTF-8 character waits for the rest. */
    CHECK_INT(decode("\xc3", false, k), 0);
}

int main(void) {
    omt_snprintf(scratch, sizeof(scratch), "/tmp/omt-tui-test-%d", (int)getpid());
    mkdir(scratch, 0700);
    RUN(paging_up_leaves_the_tail_rather_than_the_beginning);
    RUN(paging_back_down_resumes_following);
    RUN(paging_up_stops_at_the_start_of_the_log);
    RUN(paging_an_empty_log_is_harmless);
    RUN(the_log_keeps_its_capacity);
    RUN(a_prefilled_first_field_is_edited_from_its_end);
    RUN(editing_counts_characters_not_bytes);
    RUN(sd_card_job_is_local_and_uses_the_shared_validation);
    RUN(factory_alpine_keeps_an_explicit_empty_ssh_password);
    RUN(the_alpine_install_is_confirmed_before_anything_starts);
    RUN(reading_status_is_not_confirmed);
    RUN(restart_and_reboot_ask_first);
    RUN(quitting_asks_only_while_a_job_is_running);
    RUN(a_short_host_password_is_refused_by_the_precheck);
    RUN(deploy_refuses_a_remote_directory_the_core_would_reject);
    RUN(successful_alpine_setup_can_apply_the_new_pi_login);
    RUN(a_failed_job_is_reported_in_the_status_and_the_log);
    RUN(web_password_rotation_uses_its_own_fields);
    RUN(keys_switch_views_and_reveal_secrets);
    RUN(about_reproduces_the_licence_and_the_third_party_notices);
    RUN(wrapping_keeps_blank_lines_and_indentation);
    RUN(a_word_wider_than_the_view_is_broken_rather_than_clipped);
    RUN(the_document_fits_the_width_it_wrapped_to);
    RUN(the_end_of_the_notices_can_be_scrolled_to);
    RUN(a_short_terminal_scrolls_the_form_to_the_focused_row);
    RUN(a_tall_terminal_shows_every_row_at_once);
    RUN(a_narrow_terminal_shows_the_end_of_the_value_being_typed);
    RUN(the_label_gutter_leaves_room_for_a_value);
    RUN(the_visible_window_always_contains_the_focus);
    RUN(secrets_are_masked_until_revealed);
    RUN(too_small_a_terminal_says_so);
    RUN(vt_input_decodes_to_the_keys_the_app_handles);
    char command[300];
    omt_snprintf(command, sizeof(command), "rm -rf %s", scratch);
    if (system(command) != 0) fprintf(stderr, "could not remove %s\n", scratch);
    return TEST_EXIT();
}
