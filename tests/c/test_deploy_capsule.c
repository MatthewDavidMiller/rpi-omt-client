/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The capsule a deployer actually ships, held to the manifest it carries.
 * Links the real capsule, so it needs the ARM64 image built first; the other
 * deployer suites use a fixture capsule instead.
 */
#include "deploy/core/deploy.h"
#include "deploy/core/ops_internal.h"
#include "deploy/tui/ui.h"
#include "test.h"

static void the_embedded_capsule_is_exactly_the_manifest_it_ships(void) {
    const dp_capsule_member *manifest = dp_capsule_member_named("deploy/manifest-v3.txt");
    CHECK(manifest != NULL);
    if (!manifest) return;
    dp_manifest m;
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_parse_manifest((const char *)manifest->bytes, manifest->size, &m, &err));
    size_t count;
    const dp_capsule_member *members = dp_capsule_members(&count);
    CHECK_INT(m.count, count);
    for (size_t i = 0; i < m.count && i < count; i++) CHECK_STR(m.names[i], members[i].name);
    dp_manifest_free(&m);
    dp_err_free(&err);
}

/* The generator restates dp_valid_manifest_name; this is the real rule run
 * over what it accepted. */
static void every_embedded_name_is_a_safe_manifest_member(void) {
    size_t count;
    const dp_capsule_member *members = dp_capsule_members(&count);
    for (size_t i = 0; i < count; i++) {
        CHECK_MSG(dp_valid_manifest_name(members[i].name), "unsafe: %s", members[i].name);
        CHECK_MSG(members[i].size > 0, "empty: %s", members[i].name);
    }
}

static void the_capsule_carries_what_the_pi_promotes_with(void) {
    const char *required[] = {"deploy/transaction.sh", "deploy/manifest-v3.txt",
                              "deploy/host/install.sh", "deploy/host/bootstrap.sh",
                              "deploy/host/setup-sys.sh"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(required); i++) {
        CHECK_MSG(dp_capsule_member_named(required[i]) != NULL, "missing %s", required[i]);
    }
}

/* The archive is why this deployer needs nothing beside it, so "present" is
 * not enough: a truncated or misnamed build would surface on the Pi. */
static void the_image_member_is_embedded_and_is_a_gzip_archive(void) {
    const dp_capsule_member *image = dp_capsule_image();
    CHECK(image != NULL);
    if (!image) return;
    CHECK_MSG(image->size > 1024u * 1024u, "only %zu bytes", image->size);
    CHECK(image->bytes[0] == 0x1f && image->bytes[1] == 0x8b);
}

/* The deployer waits for a marker the script prints and uploads a member the
 * capsule has to contain; both are checked against the embedded bytes. */
static void the_embedded_rename_script_is_the_one_ops_drives(void) {
    const dp_capsule_member *script = dp_capsule_member_named(DP_SET_HOSTNAME_MEMBER);
    CHECK(script != NULL);
    if (!script) return;
    char *text = dp_strndup((const char *)script->bytes, script->size);
    CHECK(text && strstr(text, DP_SET_HOSTNAME_COMPLETE));
    CHECK(text && strstr(text, "rc-service omt-client restart"));
    free(text);
    const dp_capsule_member *setup = dp_capsule_member_named(DP_SETUP_SYS_MEMBER);
    text = setup ? dp_strndup((const char *)setup->bytes, setup->size) : NULL;
    CHECK(text && strstr(text, DP_SETUP_SYS_COMPLETE));
    free(text);
}

static void about_reproduces_the_shipped_licence_and_notices(void) {
    ui_lines lines = {0};
    ui_about_text(80, &lines);
    omt_buf joined;
    omt_buf_init(&joined, 1u << 24);
    for (size_t i = 0; i < lines.count; i++) {
        omt_buf_puts(&joined, lines.items[i]);
        omt_buf_putc(&joined, '\n');
    }
    const char *text = omt_buf_cstr(&joined);
    CHECK(strstr(text, "MIT License") != NULL);
    CHECK(strstr(text, "Copyright (c) 2026 Matthew David Miller") != NULL);
    CHECK(strstr(text, "Permission is hereby granted, free of charge") != NULL);
    CHECK(strstr(text, "THIRD-PARTY NOTICES") != NULL);
    CHECK(strstr(text, "SPDX license identifiers above are descriptive") != NULL);
    omt_buf_free(&joined);
    ui_lines_free(&lines);
    omt_buf report;
    omt_buf_init(&report, 1024);
    dp_err err;
    dp_err_init(&err);
    CHECK(dp_capsule_report(&report, &err));
    CHECK(strstr(omt_buf_cstr(&report), "sha256 ") != NULL);
    omt_buf_free(&report);
    dp_err_free(&err);
}

int main(void) {
    RUN(the_embedded_capsule_is_exactly_the_manifest_it_ships);
    RUN(every_embedded_name_is_a_safe_manifest_member);
    RUN(the_capsule_carries_what_the_pi_promotes_with);
    RUN(the_image_member_is_embedded_and_is_a_gzip_archive);
    RUN(the_embedded_rename_script_is_the_one_ops_drives);
    RUN(about_reproduces_the_shipped_licence_and_notices);
    return TEST_EXIT();
}
