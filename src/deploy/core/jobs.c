/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The unit of work a frontend runs on a worker thread. The sequencing here is
 * not always one call into the operations -- Deploy optionally chains a Web
 * password rotation, and several jobs validate before they connect -- and
 * holding it in one place keeps a deployment meaning the same thing whichever
 * frontend started it.
 */
#include <stdlib.h>
#include <string.h>

#include "deploy/core/deploy.h"

void dp_job_request_init(dp_job_request *r) {
    memset(r, 0, sizeof(*r));
    r->job = DP_JOB_TEST;
    dp_connection_init(&r->connection);
    r->wifi_country = dp_strdup("US");
    dp_secret_init(&r->wifi_password);
    r->wifi_connect = true;
    r->wifi_preserve_existing_profiles = true;
    dp_secret_init(&r->os_root_password);
    dp_secret_init(&r->os_pi_password);
    dp_secret_init(&r->web_password);
}

void dp_job_request_free(dp_job_request *r) {
    dp_connection_free(&r->connection);
    dp_deploy_options_free(&r->options);
    free(r->boot_directory);
    free(r->wifi_country);
    free(r->wifi_ssid);
    free(r->hostname);
    free(r->manage_hostname);
    r->boot_directory = r->wifi_country = r->wifi_ssid = r->hostname = r->manage_hostname = NULL;
    dp_secret_clear(&r->wifi_password);
    dp_secret_clear(&r->os_root_password);
    dp_secret_clear(&r->os_pi_password);
    dp_secret_clear(&r->web_password);
}

const char *dp_connection_from_fields(const dp_connection_fields *f, dp_connection *out) {
    dp_connection_init(out);
    out->host = dp_trim_dup(f->host);
    out->username = dp_trim_dup(f->username);
    char *known_hosts = dp_trim_dup(f->known_hosts);
    if (!out->host || !out->username || !known_hosts) {
        free(known_hosts);
        return "out of memory";
    }
    if (known_hosts[0]) {
        out->known_hosts_path = known_hosts;
    } else {
        free(known_hosts);
    }
    /* Sent even when empty: untouched factory Alpine accepts root with an
     * empty SSH password, so "" is an explicit credential here. */
    const char *problem = dp_secret_assign_cstr(&out->password, f->password);
    if (!problem && f->sudo_password[0]) {
        problem = dp_secret_assign_cstr(&out->sudo_password, f->sudo_password);
    }
    if (!problem && f->bootstrap_root_password[0]) {
        problem = dp_secret_assign_cstr(&out->bootstrap_root_password, f->bootstrap_root_password);
    }
    if (!problem) problem = dp_validate_connection(out);
    return problem;
}

static bool fail_text(dp_err *err, const char *problem) {
    dp_fail(err, "%s", problem);
    return false;
}

static const dp_connection *remote(const dp_job_request *r, dp_err *err) {
    if (r->has_connection) return &r->connection;
    dp_fail(err, "no connection was prepared for this operation");
    return NULL;
}

static bool rotate_web_password(dp_job_request *r, const dp_connection *c, const dp_cancel *cancel,
                                const dp_progress *p, dp_err *err) {
    const char *problem = dp_validate_web_password(&r->web_password);
    if (problem) return fail_text(err, problem);
    return dp_change_web_password(c, &r->web_password, cancel, p, err);
}

bool dp_run_job(dp_job_request *r, const dp_cancel *cancel, const dp_progress *p, dp_err *err) {
    const dp_connection *c = NULL;
    switch (r->job) {
    case DP_JOB_PREPARE_SD: {
        dp_sd_settings s = {r->boot_directory, r->wifi_country, r->wifi_ssid, r->wifi_password};
        return dp_prepare_sd_card(&s, cancel, p, err);
    }
    case DP_JOB_TEST: return (c = remote(r, err)) && dp_test_connection(c, cancel, p, err);
    case DP_JOB_ALPINE: {
        if (!(c = remote(r, err))) return false;
        dp_alpine_settings a;
        memset(&a, 0, sizeof(a));
        a.hostname = r->hostname;
        a.has_wifi = r->wifi_ssid && r->wifi_ssid[0];
        if (a.has_wifi) {
            a.wifi.ssid = r->wifi_ssid;
            a.wifi.password = r->wifi_password;
            a.wifi.connect = false;
            a.wifi.preserve_existing_profiles = true;
        }
        a.root_password = r->os_root_password;
        a.pi_password = r->os_pi_password;
        return dp_alpine_setup(c, &a, r->options.project_root, cancel, p, err);
    }
    case DP_JOB_DEPLOY:
        if (!(c = remote(r, err)) || !dp_deploy(c, &r->options, cancel, p, err)) return false;
        if (!r->rotate_web_password) return true;
        return rotate_web_password(r, c, cancel, p, err);
    case DP_JOB_MANAGE: return (c = remote(r, err)) && dp_manage(c, r->action, cancel, p, err);
    case DP_JOB_WEB_PASSWORD:
        return (c = remote(r, err)) && rotate_web_password(r, c, cancel, p, err);
    case DP_JOB_HOSTNAME:
        return (c = remote(r, err)) &&
               dp_set_hostname(c, r->manage_hostname ? r->manage_hostname : "",
                               r->options.project_root, cancel, p, err);
    case DP_JOB_WIFI: {
        dp_wifi_settings w = {r->wifi_ssid, r->wifi_password, r->wifi_connect,
                              r->wifi_preserve_existing_profiles};
        const char *problem = dp_validate_wifi(&w);
        if (problem) return fail_text(err, problem);
        return (c = remote(r, err)) && dp_apply_wifi(c, &w, cancel, p, err);
    }
    }
    return fail_text(err, "unknown job");
}
