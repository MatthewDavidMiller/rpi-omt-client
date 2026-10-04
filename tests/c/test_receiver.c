/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Port of the omt-receiver crate's unit tests: the channel's timing contract
 * against live loopback sockets, the playout queue, connector selection over
 * a scratch sysfs tree, the scaler, mode selection, decode classification,
 * the running details, the interleaver, discovery parsing, and the D-Bus
 * marshalling.
 */
#include <arpa/inet.h>
#include <netinet/in.h>
#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/proc.h"
#include "receiver/audio_alsa.h"
#include "receiver/connector.h"
#include "receiver/dbus.h"
#include "receiver/discovery.h"
#include "receiver/jitter.h"
#include "receiver/play.h"
#include "receiver/scale.h"
#include "receiver/video_drm.h"
#include "test.h"

static void sleep_ms(unsigned ms) { usleep(ms * 1000u); }

/* ------------------------------------------------------------- channel */

typedef struct {
    int listener;
    unsigned header_delay_ms; /* pause between header and payload */
    bool send_header;
    bool send_one_byte; /* send one payload byte then stall */
    unsigned hold_ms;
} server_plan;

static int listen_loopback(uint16_t *port) {
    *port = 0;
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) return -1;
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&a, sizeof(a)) != 0 || listen(fd, 4) != 0) {
        close(fd);
        return -1;
    }
    socklen_t len = sizeof(a);
    getsockname(fd, (struct sockaddr *)&a, &len);
    *port = ntohs(a.sin_port);
    return fd;
}

static void *serve(void *raw) {
    server_plan *p = raw;
    int fd = accept(p->listener, NULL, NULL);
    if (fd < 0) return NULL;
    omt_buf frame;
    omt_buf_init(&frame, 1024);
    if (!omt_build_metadata("<OMTPayload/>", 13, 7, &frame, NULL)) return NULL;
    if (p->send_header) {
        (void)!write(fd, frame.data, OMT_HEADER_SIZE);
        if (p->send_one_byte) {
            (void)!write(fd, "x", 1);
        } else {
            sleep_ms(p->header_delay_ms);
            (void)!write(fd, frame.data + OMT_HEADER_SIZE, frame.len - OMT_HEADER_SIZE);
        }
    }
    sleep_ms(p->hold_ms);
    close(fd);
    omt_buf_free(&frame);
    return NULL;
}

static bool connect_to(omt_channel *c, uint16_t port) {
    omt_endpoint ep = {"127.0.0.1", port};
    omt_err err;
    return omt_channel_connect(c, &ep, OMT_FRAME_METADATA, omt_now_ms() + 5000, &err);
}

static void run_payload_case(unsigned delay_ms, bool expect_ok) {
    uint16_t port = 0;
    server_plan plan = {listen_loopback(&port), delay_ms, true, false, 500};
    pthread_t t;
    pthread_create(&t, NULL, serve, &plan);
    omt_channel c;
    omt_channel_init(&c);
    CHECK(connect_to(&c, port));
    omt_err err;
    omt_recv_status st = omt_channel_receive(&c, omt_now_ms() + 50, &err);
    if (expect_ok) {
        CHECK_MSG(st == OMT_RECV_OK, "status %d: %s", st, err.msg);
        CHECK_INT(c.frame.header.timestamp, 7);
        CHECK(c.frame.len == 13 && memcmp(c.frame.payload, "<OMTPayload/>", 13) == 0);
        CHECK(omt_channel_connected(&c));
    }
    omt_channel_free(&c);
    pthread_join(t, NULL);
    close(plan.listener);
}

static void a_payload_arriving_after_the_slice_is_received(void) { run_payload_case(300, true); }

static void a_payload_paused_for_three_and_a_half_seconds_completes(void) {
    run_payload_case(3500, true);
}

static void an_idle_connected_socket_would_block_without_closing(void) {
    uint16_t port = 0;
    server_plan plan = {listen_loopback(&port), 0, false, false, 500};
    pthread_t t;
    pthread_create(&t, NULL, serve, &plan);
    omt_channel c;
    omt_channel_init(&c);
    CHECK(connect_to(&c, port));
    omt_err err;
    CHECK_INT(omt_channel_receive(&c, omt_now_ms() + 50, &err), OMT_RECV_WOULD_BLOCK);
    CHECK(omt_channel_connected(&c));
    omt_channel_free(&c);
    pthread_join(t, NULL);
    close(plan.listener);
}

static void a_body_that_never_finishes_is_truncated(void) {
    uint16_t port = 0;
    server_plan plan = {listen_loopback(&port), 0, true, true, OMT_BODY_BUDGET_MS + 2000};
    pthread_t t;
    pthread_create(&t, NULL, serve, &plan);
    omt_channel c;
    omt_channel_init(&c);
    CHECK(connect_to(&c, port));
    omt_err err;
    CHECK_INT(omt_channel_receive(&c, omt_now_ms() + 50, &err), OMT_RECV_TIMED_OUT);
    CHECK(strstr(err.msg, "truncated by a timeout") != NULL);
    CHECK(!omt_channel_connected(&c));
    omt_channel_free(&c);
    pthread_join(t, NULL);
    close(plan.listener);
}

static void deadlines_and_media_slices(void) {
    CHECK_INT(omt_remaining_ms(omt_now_ms() - 5000), 0);
    CHECK(omt_remaining_ms(omt_body_deadline(omt_now_ms() - 5000)) > 1000);
    uint64_t generous = omt_now_ms() + OMT_BODY_BUDGET_MS + 30000;
    CHECK(omt_body_deadline(generous) == generous);
    CHECK(OMT_BODY_BUDGET_MS > 3000 && OMT_BODY_BUDGET_MS < 8000);
    omt_frame f = {0};
    uint8_t payload[100];
    f.payload = payload;
    f.len = 100;
    f.header.metadata_length = 10;
    const uint8_t *m;
    size_t len;
    CHECK(omt_frame_media(&f, 32, &m, &len) && len == 58);
    CHECK(omt_frame_media(&f, 0, &m, &len) && len == 90);
    f.len = 32;
    f.header.metadata_length = 0;
    CHECK(omt_frame_media(&f, 32, &m, &len) && len == 0);
    f.len = 8;
    f.header.metadata_length = 10;
    CHECK(!omt_frame_media(&f, 0, &m, &len));
    f.len = 40;
    CHECK(!omt_frame_media(&f, 32, &m, &len));
    f.len = 100;
    CHECK(!omt_frame_media(&f, SIZE_MAX, &m, &len));
}

static void resolve_drops_an_ipv6_zone(void) {
    struct sockaddr_storage addrs[OMT_MAX_ADDRESSES];
    omt_err err;
    omt_endpoint scoped = {"fe80::1%eth0", 6400};
    CHECK_INT(omt_endpoint_resolve(&scoped, addrs, OMT_MAX_ADDRESSES, &err), 1);
    CHECK(addrs[0].ss_family == AF_INET6);
    CHECK_INT(ntohs(((struct sockaddr_in6 *)&addrs[0])->sin6_port), 6400);
    omt_endpoint literal = {"127.0.0.1", 1};
    CHECK_INT(omt_endpoint_resolve(&literal, addrs, OMT_MAX_ADDRESSES, &err), 1);
    omt_endpoint bad = {"not a host", 1};
    CHECK_INT(omt_endpoint_resolve(&bad, addrs, OMT_MAX_ADDRESSES, &err), 0);
}

static void a_failed_reconnect_leaves_the_channel_down(void) {
    uint16_t port = 0;
    int fd = listen_loopback(&port);
    close(fd);
    omt_channel c;
    omt_channel_init(&c);
    omt_endpoint ep = {"127.0.0.1", port};
    omt_err err;
    CHECK(!omt_channel_connect(&c, &ep, OMT_FRAME_VIDEO, omt_now_ms() + OMT_RECOVER_TIMEOUT_MS,
                               &err));
    CHECK(!omt_channel_connected(&c));
    omt_channel_free(&c);
}

/* -------------------------------------------------------------- jitter */

static omt_frame video_frame(size_t payload, int32_t n, int32_t d) {
    omt_frame f = {0};
    f.header.frame_type = OMT_FRAME_VIDEO;
    f.has_video = true;
    f.video.width = 1920;
    f.video.height = 1080;
    f.video.frame_rate_n = n;
    f.video.frame_rate_d = d;
    f.len = OMT_VIDEO_HEADER_SIZE + payload;
    f.cap = f.len;
    f.payload = test_alloc(f.len);
    return f;
}

static omt_frame audio_frame(int32_t samples, int32_t rate) {
    omt_frame f = {0};
    f.header.frame_type = OMT_FRAME_AUDIO;
    f.has_audio = true;
    f.audio.sample_rate = rate;
    f.audio.samples_per_channel = samples;
    f.audio.channels = 2;
    f.audio.active_channels = 3;
    f.len = 24;
    f.cap = 24;
    f.payload = test_alloc(24);
    return f;
}

static uint64_t push(omt_queue *q, omt_frame f, bool playing) {
    return omt_queue_push(q, &f, playing);
}

static void queue_contract(void) {
    omt_queue q;
    omt_queue_init(&q, OMT_QUEUE_VIDEO, 0, OMT_VIDEO_BYTE_CAP);
    CHECK(!omt_queue_filled(&q));
    CHECK_INT(push(&q, video_frame(32, 30, 1), false), 0);
    CHECK(omt_queue_filled(&q));
    CHECK(push(&q, video_frame(32, 30, 1), true) >= 1);
    CHECK_INT(q.count, 1);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_VIDEO, 4000000000ull, OMT_VIDEO_BYTE_CAP);
    for (int i = 0; i < 4; i++) {
        CHECK(!omt_queue_filled(&q));
        (void)push(&q, video_frame(64, 1, 1), false);
    }
    CHECK(omt_queue_filled(&q));
    CHECK(omt_queue_duration_ns(&q) == 4000000000ull);
    omt_frame out;
    for (int i = 0; i < 4; i++) {
        CHECK(omt_queue_pop(&q, &out));
        omt_frame_free(&out);
    }
    CHECK(!omt_queue_pop(&q, &out));
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_VIDEO, 8000000000ull, 250);
    CHECK_INT(push(&q, video_frame(80, 1, 1), false), 0);
    CHECK_INT(push(&q, video_frame(80, 1, 1), false), 0);
    CHECK(push(&q, video_frame(80, 1, 1), false) >= 1);
    CHECK(q.bytes <= 250 && q.count <= 2);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_VIDEO, 4000000000ull, OMT_VIDEO_BYTE_CAP);
    omt_frame meta = {0};
    meta.header.frame_type = OMT_FRAME_METADATA;
    CHECK_INT(omt_queue_push(&q, &meta, false), 0);
    CHECK(omt_queue_empty(&q) && q.bytes == 0);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_VIDEO, 2000000000ull, OMT_VIDEO_BYTE_CAP);
    (void)push(&q, video_frame(32, 1, 1), false);
    (void)push(&q, video_frame(32, 1, 1), false);
    CHECK(omt_queue_filled(&q));
    (void)push(&q, video_frame(32, 1, 1), true);
    CHECK(omt_queue_duration_ns(&q) <= 2000000000ull);
    CHECK_INT(q.count, 2);
    omt_queue_free(&q);
}

static void audio_queue_contract(void) {
    omt_audio_header h = {0, 48000, 48000, 2, 3};
    uint64_t ns;
    CHECK(omt_audio_interval_ns(&h, &ns) && ns == 1000000000ull);
    omt_queue q;
    omt_queue_init(&q, OMT_QUEUE_AUDIO, 2000000000ull, OMT_AUDIO_BYTE_CAP);
    (void)push(&q, audio_frame(48000, 48000), false);
    (void)push(&q, audio_frame(48000, 48000), false);
    CHECK(omt_queue_filled(&q));
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_AUDIO, 0, OMT_AUDIO_BYTE_CAP);
    (void)push(&q, audio_frame(960, 48000), false);
    CHECK(omt_queue_filled(&q));
    omt_queue_keep_latest_only(&q);
    for (int i = 0; i < 4; i++) CHECK_INT(push(&q, audio_frame(960, 48000), true), 0);
    CHECK_INT(q.count, 5);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_AUDIO, 0, OMT_AUDIO_BYTE_CAP);
    uint64_t dropped = 0;
    for (int i = 0; i < 12; i++) dropped += push(&q, audio_frame(960, 48000), true);
    CHECK_INT(q.count, 5);
    CHECK_INT(dropped, 7);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_AUDIO, 250000000ull, OMT_AUDIO_BYTE_CAP);
    for (int i = 0; i < 20; i++) (void)push(&q, audio_frame(960, 48000), true);
    CHECK_INT(q.count, 17);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_VIDEO, 0, OMT_VIDEO_BYTE_CAP);
    (void)push(&q, video_frame(32, 30, 1), false);
    (void)push(&q, video_frame(32, 30, 1), false);
    omt_queue_keep_latest_only(&q);
    CHECK_INT(q.count, 1);
    omt_queue_free(&q);

    omt_queue_init(&q, OMT_QUEUE_VIDEO, 0, OMT_VIDEO_BYTE_CAP);
    uint8_t *spare;
    size_t cap;
    omt_queue_take_spare(&q, &spare, &cap);
    CHECK(spare == NULL && cap == 0);
    for (int i = 0; i < 10; i++) (void)push(&q, video_frame(64, 30, 1), true);
    CHECK_INT(q.spare_count, OMT_SPARE_BUFFERS);
    omt_queue_take_spare(&q, &spare, &cap);
    CHECK(cap >= 64);
    free(spare);
    omt_queue_recycle(&q, NULL, 0);
    CHECK_INT(q.spare_count, OMT_SPARE_BUFFERS - 1);
    omt_queue_free(&q);

    omt_fill_gate gate;
    omt_gate_init(&gate);
    CHECK(!omt_gate_audio_ready(&gate));
    omt_gate_set_video(&gate, true);
    CHECK(omt_gate_video_ready(&gate) && !omt_gate_audio_ready(&gate));
    omt_gate_set_audio_missing(&gate);
    CHECK(omt_gate_audio_ready(&gate));
}

/* ----------------------------------------------------------- connector */

static char tree[256];

static void put_file(const char *rel, const char *content) {
    char path[512];
    snprintf(path, sizeof(path), "%s/%s", tree, rel);
    char dir[512];
    snprintf(dir, sizeof(dir), "%s", path);
    *strrchr(dir, '/') = 0;
    omt_err err;
    CHECK(omt_mkdir_all(dir, 0755, &err));
    FILE *f = fopen(path, "w");
    if (f) {
        fputs(content, f);
        fclose(f);
    }
}

static void card(const char *name, const char *status, const char *id) {
    char rel[128];
    char dir[512];
    snprintf(dir, sizeof(dir), "%s/sys/%s", tree, name);
    omt_err err;
    CHECK(omt_mkdir_all(dir, 0755, &err));
    if (status) {
        snprintf(rel, sizeof(rel), "sys/%s/status", name);
        put_file(rel, status);
    }
    if (id) {
        snprintf(rel, sizeof(rel), "sys/%s/connector_id", name);
        put_file(rel, id);
    }
}

static bool find_in(const char *name, omt_hdmi *out) {
    char sys[300], dev[300], snd[300];
    snprintf(sys, sizeof(sys), "%s/sys", tree);
    snprintf(dev, sizeof(dev), "%s/dev", tree);
    snprintf(snd, sizeof(snd), "%s/sound", tree);
    return omt_hdmi_named_in(sys, dev, snd, name, out);
}

static void reset_tree(const char *label) {
    char cmd[600];
    snprintf(cmd, sizeof(cmd), "rm -rf '%s'", tree);
    if (tree[0]) (void)!system(cmd);
    const char *t = getenv("TMPDIR");
    snprintf(tree, sizeof(tree), "%s/omt-connector-%s-%d", t ? t : "/tmp", label, (int)getpid());
    char sub[300];
    omt_err err;
    snprintf(sub, sizeof(sub), "%s/sys", tree);
    CHECK(omt_mkdir_all(sub, 0755, &err));
    snprintf(sub, sizeof(sub), "%s/dev", tree);
    CHECK(omt_mkdir_all(sub, 0755, &err));
    snprintf(sub, sizeof(sub), "%s/sound", tree);
    CHECK(omt_mkdir_all(sub, 0755, &err));
}

static void connector_selection(void) {
    omt_hdmi c;
    reset_tree("order");
    card("card0-HDMI-A-1", "connected", "32");
    card("card1-HDMI-A-1", "connected", "48");
    put_file("dev/card0", "");
    put_file("dev/card1", "");
    CHECK(find_in("HDMI-A-1", &c));
    CHECK_INT(c.id, 32);
    CHECK_STR(c.alsa_device, "plughw:CARD=vc4hdmi0,DEV=0");

    reset_tree("unreadable");
    card("card0-HDMI-A-1", NULL, "32");
    card("card1-HDMI-A-1", "connected", "48");
    put_file("dev/card0", "");
    put_file("dev/card1", "");
    CHECK(find_in("HDMI-A-1", &c) && c.id == 48);

    reset_tree("skip");
    card("card0-HDMI-A-2", "disconnected", "32");
    card("card1-HDMI-A-2", "connected", NULL);
    card("card2-HDMI-A-2", "connected", "0");
    card("card3-HDMI-A-2", "connected", "64");
    card("card4-HDMI-A-2", "connected", "80");
    put_file("dev/card0", "");
    put_file("dev/card1", "");
    put_file("dev/card2", "");
    put_file("dev/card4", "");
    CHECK(find_in("HDMI-A-2", &c) && c.id == 80);
    CHECK_STR(c.alsa_device, "plughw:CARD=vc4hdmi1,DEV=0");

    reset_tree("none");
    card("card0-HDMI-A-1", "disconnected", "32");
    put_file("dev/card0", "");
    CHECK(!find_in("HDMI-A-1", &c));
    CHECK(!find_in("HDMI-A-2", &c));

    reset_tree("renumber");
    card("card0-HDMI-A-1", "connected", "32");
    put_file("dev/card0", "");
    CHECK(find_in("HDMI-A-1", &c));
    CHECK(omt_hdmi_is_connected(&c));
    card("card0-HDMI-A-1", "connected", "48");
    CHECK(!omt_hdmi_is_connected(&c));
    card("card0-HDMI-A-1", "disconnected", "32");
    CHECK(!omt_hdmi_is_connected(&c));

    char alsa[96], snd[300];
    reset_tree("audio");
    snprintf(snd, sizeof(snd), "%s/sound", tree);
    put_file("sound/card0/id", "vc4hdmi0");
    put_file("sound/card1/id", "vc4hdmi1");
    omt_hdmi_alsa_device_in(snd, "HDMI-A-1", alsa);
    CHECK_STR(alsa, "plughw:CARD=vc4hdmi0,DEV=0");
    omt_hdmi_alsa_device_in(snd, "HDMI-A-2", alsa);
    CHECK_STR(alsa, "plughw:CARD=vc4hdmi1,DEV=0");
    reset_tree("single");
    snprintf(snd, sizeof(snd), "%s/sound", tree);
    put_file("sound/card0/id", "vc4hdmi");
    omt_hdmi_alsa_device_in(snd, "HDMI-A-1", alsa);
    CHECK_STR(alsa, "plughw:CARD=vc4hdmi,DEV=0");
    omt_hdmi_alsa_device_in(snd, "HDMI-A-2", alsa);
    CHECK_STR(alsa, "plughw:CARD=vc4hdmi,DEV=0");
    reset_tree("mixed");
    snprintf(snd, sizeof(snd), "%s/sound", tree);
    put_file("sound/card0/id", "Headphones");
    put_file("sound/card1/id", "vc4hdmi0");
    put_file("sound/card2/id", "vc4hdmi1");
    omt_hdmi_alsa_device_in(snd, "HDMI-A-2", alsa);
    CHECK_STR(alsa, "plughw:CARD=vc4hdmi1,DEV=0");
    omt_hdmi_alsa_device_in("/nonexistent-omt-sound-root", "HDMI-A-2", alsa);
    CHECK_STR(alsa, "plughw:CARD=vc4hdmi1,DEV=0");
    CHECK(!omt_hdmi_find("HDMI-A-9", &c));
    CHECK(!omt_hdmi_find("../../etc", &c));
    char cmd[600];
    snprintf(cmd, sizeof(cmd), "rm -rf '%s'", tree);
    (void)!system(cmd);
}

/* --------------------------------------------------------------- scale */

static void scaler_contract(void) {
    omt_placement p;
    CHECK(omt_placement_fit(1920, 1080, 1280, 720, &p));
    CHECK(p.x == 0 && p.y == 0 && p.width == 1280 && p.height == 720);
    CHECK(omt_placement_fills(&p, 1280, 720));
    CHECK(omt_placement_fit(1920, 1080, 1024, 768, &p));
    CHECK(p.x == 0 && p.y == 96 && p.width == 1024 && p.height == 576);
    CHECK(omt_placement_fit(640, 480, 1280, 720, &p));
    CHECK(p.x == 160 && p.y == 0 && p.width == 960 && p.height == 720);
    CHECK(!omt_placement_fit(0, 1080, 1280, 720, &p));
    CHECK(!omt_placement_fit(1920, 1080, 1280, 0, &p));
    CHECK(omt_placement_fit(1920, 16, 64, 64, &p) && p.width >= 1 && p.height >= 1);
    CHECK(omt_scale_sample(0, 2, 4) == 1 && omt_scale_sample(1, 2, 4) == 3);
    size_t reduced[4] = {0, 2, 3, 5};
    for (size_t i = 0; i < 4; i++) CHECK_INT(omt_scale_sample(i, 4, 6), reduced[i]);

    uint8_t source[64], dst[64];
    for (int i = 0; i < 64; i++) source[i] = (uint8_t)i;
    omt_scaler s;
    omt_err err;
    CHECK(omt_placement_fit(4, 4, 4, 4, &p));
    CHECK(omt_scaler_init(&s, 4, 4, 16, p, &err));
    CHECK(omt_scaler_render(&s, source, 64, dst, 64, 16, &err));
    CHECK(memcmp(source, dst, 64) == 0);
    CHECK(!omt_scaler_render(&s, source, 64, dst, 63, 16, &err));
    omt_scaler_free(&s);
}

/* --------------------------------------------------------- mode select */

static omt_mode_shape shape(uint16_t w, uint16_t h, double r) {
    omt_mode_shape s = {w, h, r, false};
    return s;
}

static bool choose(const omt_mode_shape *s, size_t n, uint16_t w, uint16_t h, double r,
                   size_t index, bool scaled) {
    omt_mode_choice c;
    return omt_choose_mode(s, n, w, h, r, &c) && c.index == index && c.scaled == scaled;
}

static void mode_selection(void) {
    omt_mode_shape a[] = {{1920, 1080, 59.94, true},
                          shape(1920, 1080, 50),
                          shape(1920, 1080, 59.94),
                          shape(1920, 1080, 60),
                          shape(1280, 720, 59.94)};
    CHECK(choose(a, 5, 1920, 1080, 59.94, 2, false));
    CHECK(choose(a, 5, 1920, 1080, 60.0, 3, false));
    CHECK(choose(a, 5, 1920, 1080, 50.0, 1, false));
    CHECK(choose(a, 5, 1280, 720, 59.94, 4, false));
    CHECK(choose(a, 5, 1920, 1080, 29.97, 3, false));
    omt_mode_choice c;
    CHECK(!omt_choose_mode(a, 1, 1920, 1080, 59.94, &c));
    CHECK(!omt_choose_mode(a, 0, 1920, 1080, 60, &c));

    omt_mode_shape b[] = {shape(1280, 720, 60),
                          shape(1280, 720, 59.94),
                          shape(800, 600, 60),
                          shape(640, 480, 60),
                          {1920, 1080, 60, true}};
    CHECK(choose(b, 5, 1920, 1080, 30, 0, true));
    CHECK(choose(b, 5, 1920, 1080, 59.94, 1, true));
    CHECK(choose(b, 5, 1280, 720, 60, 0, false));

    omt_mode_shape d[] = {shape(1920, 1080, 60), shape(1280, 720, 60), shape(3840, 2160, 60)};
    CHECK(choose(d, 3, 640, 480, 60, 1, true));
    CHECK(choose(d, 3, 1920, 1200, 60, 0, true));
    CHECK(!omt_choose_mode(d + 2, 1, 1920, 1080, 60, &c));

    omt_mode_shape e[] = {shape(1920, 1080, 24), shape(1920, 1080, 50)};
    CHECK(choose(e, 2, 1920, 1080, 25, 1, false));
    CHECK(choose(e, 2, 1920, 1080, 24, 0, false));
    omt_mode_shape f[] = {shape(1920, 1080, 59.9401), shape(1920, 1080, 60)};
    CHECK(choose(f, 2, 1920, 1080, 59.94, 0, false));
    CHECK(choose(f, 2, 1920, 1080, 59.9, 1, false));
    omt_mode_shape g[] = {shape(1920, 1080, 0)};
    CHECK(!omt_choose_mode(g, 1, 1920, 1080, 60, &c));
}

static void decode_classification(void) {
    vmx_status all[] = {VMX_INVALID_DIMENSIONS, VMX_EMPTY,          VMX_OVERSIZED,
                        VMX_TRUNCATED,          VMX_INVALID_FORMAT, VMX_UNSUPPORTED_FORMAT,
                        VMX_SLICE_COUNT,        VMX_OUTPUT_SIZE,    VMX_WORKER_FAILURE,
                        VMX_CORRUPT_STREAM};
    char detail[256];
    for (size_t i = 0; i < OMT_ARRAY_LEN(all); i++) {
        bool local = all[i] == VMX_TRUNCATED || all[i] == VMX_INVALID_FORMAT ||
                     all[i] == VMX_OVERSIZED || all[i] == VMX_SLICE_COUNT ||
                     all[i] == VMX_CORRUPT_STREAM;
        uint32_t skips = 0;
        omt_present p = omt_classify_decode(all[i], true, &skips, detail, sizeof(detail));
        if (p == OMT_PRESENT_SKIPPED) CHECK(local);
        if (p == OMT_PRESENT_UNSUPPORTED) CHECK(all[i] == VMX_UNSUPPORTED_FORMAT);
        if (p == OMT_PRESENT_FAILED) CHECK(!local && all[i] != VMX_UNSUPPORTED_FORMAT);
        uint32_t before = 0;
        p = omt_classify_decode(all[i], false, &before, detail, sizeof(detail));
        CHECK(p != OMT_PRESENT_SKIPPED && before == 0);
    }
    uint32_t skips = 0;
    for (uint32_t held = 1; held <= OMT_SKIP_BUDGET; held++) {
        CHECK_INT(omt_classify_decode(VMX_CORRUPT_STREAM, true, &skips, detail, sizeof(detail)),
                  OMT_PRESENT_SKIPPED);
        CHECK_INT(skips, held);
    }
    CHECK_INT(omt_classify_decode(VMX_CORRUPT_STREAM, true, &skips, detail, sizeof(detail)),
              OMT_PRESENT_FAILED);
    CHECK_STR(detail, "VMX decoder rejected 21 consecutive frames: CorruptStream");

    omt_describe_presentation(false, 1920, 1080, true, 1280, 720, detail, sizeof(detail));
    CHECK_STR(detail, "Playing OMT video. Scaled from 1920x1080 to the display's 1280x720 mode.");
    omt_describe_presentation(true, 1280, 720, false, 0, 0, detail, sizeof(detail));
    CHECK_STR(detail, "Playing interlaced input progressively without deinterlacing.");
}

/* ------------------------------------------------------------- details */

static void running_details(void) {
    char out[512];
    const char *base = "Playing OMT video.";
    omt_describe_running(base, 0, 0, 0, 0, 0, out, sizeof(out));
    CHECK_STR(out, base);
    omt_describe_running(base, 0, 0, 2, 0, 0, out, sizeof(out));
    CHECK_STR(out, "Playing OMT video. 2 video reconnect(s) in this session.");
    omt_describe_running(base, 0, 0, 0, 3, 0, out, sizeof(out));
    CHECK_STR(out, "Playing OMT video. 3 skipped frame(s) in this session.");
    omt_describe_running(base, 0, 0, 2, 3, 0, out, sizeof(out));
    CHECK_STR(out,
              "Playing OMT video. 2 video reconnect(s) and 3 skipped frame(s) in this session.");
    omt_describe_running(base, 4000, 4000, 0, 0, 0, out, sizeof(out));
    CHECK_STR(out, "Playing OMT video. 4000 ms playout delay (~4000 ms buffered).");
    omt_describe_running(base, 250, 233, 0, 0, 0, out, sizeof(out));
    CHECK_STR(out, "Playing OMT video. 250 ms playout delay (~200 ms buffered).");
    omt_describe_running(base, 4000, 1000, 0, 0, 2, out, sizeof(out));
    CHECK(strstr(out, "4000 ms playout delay") && strstr(out, "2 buffer underrun(s)") &&
          !strstr(out, "skipped frame"));
    omt_describe_running(base, 0, 0, UINT64_MAX, 0, 0, out, sizeof(out));
    CHECK(strstr(out, "18446744073709551615") != NULL);

    omt_running_detail cache;
    memset(&cache, 0, sizeof(cache));
    const char *p = omt_running_detail_get(&cache, base, 500, 433, 0, 0, 0);
    CHECK(strstr(p, "~400 ms buffered") != NULL);
    p = omt_running_detail_get(&cache, base, 500, 466, 0, 0, 0);
    CHECK(strstr(p, "~400 ms buffered") != NULL);
    p = omt_running_detail_get(&cache, base, 500, 500, 0, 0, 0);
    CHECK(strstr(p, "~500 ms buffered") != NULL);

    omt_describe_audio(0, out, sizeof(out));
    CHECK_STR(out, "Playing OMT video and audio.");
    omt_describe_audio(7, out, sizeof(out));
    CHECK(omt_has_prefix(out, "Playing OMT video and audio.") && strchr(out, '7'));

    /* The reconnect budget's bounds, pinned as the Rust suite did. */
    uint64_t worst = 0, refused = 0;
    for (uint32_t a = 0; a < OMT_RECOVER_ATTEMPTS; a++) {
        worst += OMT_RECOVER_TIMEOUT_MS + OMT_RECOVER_BACKOFF_MS * a;
        refused += OMT_RECOVER_BACKOFF_MS * a;
    }
    CHECK(OMT_MEDIA_STALL_MS > OMT_MEDIA_GRACE_MS && OMT_MEDIA_STALL_MS > worst);
    CHECK(OMT_MEDIA_STALL_MS > OMT_RECEIVE_SLICE_MS * 10 && OMT_MEDIA_STALL_MS <= 30000);
    CHECK(worst <= 4000 && OMT_RECOVER_TIMEOUT_MS < OMT_CONNECT_TIMEOUT_MS && refused < 1000);
}

/* --------------------------------------------------------------- audio */

static void put_floats(uint8_t *out, const float *values, size_t n) {
    for (size_t i = 0; i < n; i++) {
        uint32_t bits;
        memcpy(&bits, &values[i], 4);
        omt_put_le32(out + 4 * i, bits);
    }
}

static void audio_contract(void) {
    CHECK_INT(omt_audio_prefill_frames(48000, 11520, 960), 4800);
    CHECK_INT(omt_audio_prefill_frames(44100, 10584, 882), 4410);
    CHECK_INT(omt_audio_prefill_frames(48000, 512, 128), 512);
    CHECK_INT(omt_audio_prefill_frames(48000, 16384, 8192), 8192);
    CHECK_INT(omt_audio_prefill_frames(48000, 0, 0), 1);
    CHECK_INT(omt_audio_prefill_frames(0, 11520, 960), 960);
    CHECK_INT(omt_audio_prefill_frames(-1, 11520, 960), 960);
    CHECK_INT(omt_audio_target_frames(48000, 11520, 4800), 7680);
    CHECK_INT(omt_audio_target_frames(48000, 11520, 9000), 9000);
    CHECK_INT(omt_audio_target_frames(48000, 4000, 1000), 4000);
    CHECK_INT(omt_audio_target_frames(0, 11520, 960), 960);

    omt_err err;
    uint8_t body[128];
    float out[33];
    float v6[] = {1, 2, 3, -1, -2, -3};
    put_floats(body, v6, 6);
    CHECK(omt_audio_interleave(body, 24, 3, 3, 2, out, 6, &err));
    float e6[] = {1, -1, 2, -2, 3, -3};
    CHECK(memcmp(out, e6, sizeof(e6)) == 0);
    float v2[] = {7, 8};
    put_floats(body, v2, 2);
    CHECK(omt_audio_interleave(body, 8, 2, 2, 2, out, 4, &err));
    float e2[] = {0, 7, 0, 8};
    CHECK(memcmp(out, e2, sizeof(e2)) == 0);
    CHECK(omt_audio_interleave(body, 8, 1, 2, 2, out, 4, &err));
    float e3[] = {7, 0, 8, 0};
    CHECK(memcmp(out, e3, sizeof(e3)) == 0);
    CHECK(omt_audio_interleave(NULL, 0, 0, 2, 2, out, 4, &err));
    CHECK(!omt_audio_interleave(body, 12, 3, 2, 2, out, 4, &err));
    CHECK(!omt_audio_interleave(body, 16, 3, 2, 2, out, 3, &err));
    float ones[32];
    for (int i = 0; i < 32; i++) ones[i] = 1.0f;
    put_floats(body, ones, 32);
    CHECK(omt_audio_interleave(body, 128, UINT32_MAX, 1, 33, out, 33, &err));
    CHECK(out[32] == 0.0f && out[0] == 1.0f && out[31] == 1.0f);
}

/* ----------------------------------------------------------- discovery */

static void discovery_parsing(void) {
    omt_endpoint ep;
    CHECK(omt_endpoint_from_parts("192.0.2.10", 6400, &ep) && !strcmp(ep.host, "192.0.2.10"));
    CHECK(omt_endpoint_from_parts("2001:db8::1", 6400, &ep));
    CHECK(!omt_endpoint_from_parts("192.0.2.10", 0, &ep));
    CHECK(!omt_endpoint_from_parts("not a host", 6400, &ep));
    CHECK(!omt_endpoint_from_parts("192.0.2.10/../x", 6400, &ep));
    const char *refused[] = {
        "fe80::e65f:1ff:fe2e:9f04", "febf::1", "::", "ff02::1", "0.0.0.0", "239.0.0.1"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(refused); i++)
        CHECK_MSG(!omt_endpoint_from_parts(refused[i], 6400, &ep), "%s", refused[i]);
    CHECK(omt_endpoint_from_parts("169.254.10.20", 6400, &ep));
    CHECK(omt_endpoint_from_parts("source.example", 6400, &ep));
    CHECK(omt_endpoint_from_parts("fec0::1", 6400, &ep));

    omt_announcement a;
    const char *doc = "<OMTAddress><Name>Camera</Name><Port>6400</Port>"
                      "<Addresses><IPAddress>192.0.2.10</IPAddress></Addresses></OMTAddress>";
    CHECK(omt_announcement_read(doc, strlen(doc), &a));
    CHECK(!strcmp(a.name, "Camera") && !a.removed && a.has_endpoint && a.endpoint.port == 6400);
    const char *gone = "<OMTAddress><Name>Camera</Name><Removed>true</Removed></OMTAddress>";
    CHECK(omt_announcement_read(gone, strlen(gone), &a) && a.removed && !a.has_endpoint);
    const char *bad[] = {
        "<OMTAddress><Port>6400</Port></OMTAddress>",
        "<OMTAddress><Name>bad\xe2\x80\xaename</Name></OMTAddress>",
        "<OMTAddress><Name>Camera</Name><Port>6400</Port><Port>1</Port></OMTAddress>"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(bad); i++)
        CHECK(!omt_announcement_read(bad[i], strlen(bad[i]), &a));
    const char *zero = "<OMTAddress><Name>Camera</Name><Port>0</Port>"
                       "<IPAddress>192.0.2.10</IPAddress></OMTAddress>";
    CHECK(omt_announcement_read(zero, strlen(zero), &a) && !a.has_endpoint);
}

/* ---------------------------------------------------------------- dbus */

static void dbus_marshalling_round_trips(void) {
    omt_dbus_arg args[] = {{.type = 'i', .v.i = -1},
                           {.type = 'i', .v.i = -1},
                           {.type = 's', .v.s = "_omt._tcp"},
                           {.type = 's', .v.s = ""},
                           {.type = 'u', .v.u = 0}};
    omt_buf b;
    omt_buf_init(&b, OMT_DBUS_MAX_MESSAGE);
    CHECK(omt_dbus_build_call(&b, 7, 0, "org.freedesktop.Avahi", "/",
                              "org.freedesktop.Avahi.Server", "ServiceBrowserNew", args, 5));
    CHECK_INT(omt_dbus_message_length(b.data, b.len), b.len);
    uint8_t *raw = test_alloc(b.len);
    memcpy(raw, b.data, b.len);
    omt_dbus_message m;
    CHECK(omt_dbus_parse(raw, b.len, &m));
    CHECK_INT(m.type, OMT_DBUS_METHOD_CALL);
    CHECK_INT(m.serial, 7);
    CHECK_STR(m.member, "ServiceBrowserNew");
    CHECK_STR(m.interface, "org.freedesktop.Avahi.Server");
    CHECK_STR(m.path, "/");
    CHECK_STR(m.signature, "iissu");
    omt_dbus_reader r;
    omt_dbus_reader_init(&r, &m);
    CHECK_INT(omt_dbus_read_i32(&r), -1);
    CHECK_INT(omt_dbus_read_i32(&r), -1);
    CHECK_STR(omt_dbus_read_string(&r), "_omt._tcp");
    CHECK_STR(omt_dbus_read_string(&r), "");
    CHECK_INT(omt_dbus_read_u32(&r), 0);
    CHECK(!r.failed && r.pos == r.end);
    (void)omt_dbus_read_u32(&r);
    CHECK(r.failed);
    omt_dbus_message_free(&m);

    /* Every truncation and every single-byte corruption parses or is refused
     * without reading out of bounds; ASan watches the reads. */
    for (size_t len = 0; len < b.len; len++) {
        uint8_t *copy = test_alloc(len ? len : 1);
        memcpy(copy, b.data, len);
        omt_dbus_message t;
        CHECK(!omt_dbus_parse(copy, len, &t));
        free(copy);
    }
    for (size_t i = 0; i < b.len; i++) {
        uint8_t *copy = test_alloc(b.len);
        memcpy(copy, b.data, b.len);
        copy[i] ^= 0x5A;
        omt_dbus_message t;
        if (omt_dbus_parse(copy, b.len, &t)) {
            omt_dbus_reader tr;
            omt_dbus_reader_init(&tr, &t);
            (void)omt_dbus_read_i32(&tr);
            (void)omt_dbus_read_string(&tr);
            omt_dbus_skip_aay(&tr);
            omt_dbus_message_free(&t);
        } else {
            free(copy);
        }
    }
    omt_buf_free(&b);
}

int main(void) {
    signal(SIGPIPE, SIG_IGN);
    RUN(deadlines_and_media_slices);
    RUN(resolve_drops_an_ipv6_zone);
    RUN(a_payload_arriving_after_the_slice_is_received);
    RUN(an_idle_connected_socket_would_block_without_closing);
    RUN(a_failed_reconnect_leaves_the_channel_down);
    RUN(queue_contract);
    RUN(audio_queue_contract);
    RUN(connector_selection);
    RUN(scaler_contract);
    RUN(mode_selection);
    RUN(decode_classification);
    RUN(running_details);
    RUN(audio_contract);
    RUN(discovery_parsing);
    RUN(dbus_marshalling_round_trips);
    /* The two slow timing cases last: they wait out real budgets. */
    RUN(a_payload_paused_for_three_and_a_half_seconds_completes);
    RUN(a_body_that_never_finishes_is_truncated);
    return TEST_EXIT();
}
