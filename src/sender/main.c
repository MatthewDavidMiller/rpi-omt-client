/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * omt-test-sender: a bounded, first-party OMT source for exercising the
 * appliance receiver. The video bodies are the repository's reference-encoded
 * VMX1 conformance frames; audio is generated directly in OMT's planar FPA1
 * representation.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <math.h>
#include <netinet/in.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include "common/buf.h"
#include "common/ipaddr.h"
#include "common/proc.h"
#include "protocol/omt.h"

#define DEFAULT_PORT_FIRST 6400
#define DEFAULT_PORT_LAST 6600
#define MAX_CLIENTS 8
#define DEFAULT_FRAME_RATE 60u
/* The receiver's absolute ceiling; a sender above it could only be refused. */
#define MAX_FRAME_RATE 60u
#define OMT_TICKS_PER_SECOND 10000000ull
#define AUDIO_SAMPLE_RATE 48000
#define AUDIO_SAMPLES 800
#define AUDIO_BLOCKS_PER_SECOND (AUDIO_SAMPLE_RATE / AUDIO_SAMPLES)
#define SUBSCRIPTION_TIMEOUT_S 5
#define WRITE_TIMEOUT_S 5

/* The two reference frames, embedded at build time. The assembler computes
 * each length, so no C code subtracts pointers into different objects. */
extern const uint8_t omt_gradient_vmx[], omt_flat_vmx[];
extern const uint32_t omt_gradient_vmx_size, omt_flat_vmx_size;
__asm__(".section .rodata\n"
        ".balign 16\n"
        ".global omt_gradient_vmx\n"
        "omt_gradient_vmx:\n"
        ".incbin \"tests/vectors/vmx/gradient-1920x1080-709.vmx\"\n"
        "1:\n"
        ".balign 16\n"
        ".global omt_flat_vmx\n"
        "omt_flat_vmx:\n"
        ".incbin \"tests/vectors/vmx/flat-1920x1080-709.vmx\"\n"
        "2:\n"
        ".balign 4\n"
        ".global omt_gradient_vmx_size\n"
        "omt_gradient_vmx_size:\n"
        ".4byte 1b - omt_gradient_vmx\n"
        ".global omt_flat_vmx_size\n"
        "omt_flat_vmx_size:\n"
        ".4byte 2b - omt_flat_vmx\n"
        ".previous\n");

typedef struct {
    struct sockaddr_storage bind;
    socklen_t bind_len;
    int port; /* 0 means search the default range */
    uint32_t frame_rate;
} sender_options;

typedef enum { SUB_VIDEO, SUB_AUDIO } subscription;

static atomic_int clients;

static void usage(void) {
    printf("Usage: omt-test-sender [--bind IP] [--port PORT] [--frame-rate FPS]\n"
           "\n"
           "Streams reference VMX1 1920x1080 video and stereo FPA1 audio.\n"
           "Without --port, the first available TCP port in 6400-6600 is used.\n"
           "Without --frame-rate, video is announced and paced at 60 fps.\n");
}

static bool parse_bind(const char *value, sender_options *o) {
    uint8_t v4[4], v6[16];
    size_t len = strlen(value);
    memset(&o->bind, 0, sizeof(o->bind));
    if (omt_parse_ipv4(value, len, v4)) {
        struct sockaddr_in *a = (struct sockaddr_in *)&o->bind;
        a->sin_family = AF_INET;
        memcpy(&a->sin_addr, v4, 4);
        o->bind_len = sizeof(*a);
        return true;
    }
    if (omt_parse_ipv6(value, len, v6)) {
        struct sockaddr_in6 *a = (struct sockaddr_in6 *)&o->bind;
        a->sin6_family = AF_INET6;
        memcpy(&a->sin6_addr, v6, 16);
        o->bind_len = sizeof(*a);
        return true;
    }
    return false;
}

/* Returns 0 to run, 1 for help, 2 for version, -1 on error. */
static int parse_options(int argc, char **argv, sender_options *o, char *error, size_t size) {
    memset(o, 0, sizeof(*o));
    (void)parse_bind("0.0.0.0", o);
    o->frame_rate = DEFAULT_FRAME_RATE;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (!strcmp(a, "--help") || !strcmp(a, "-h")) return 1;
        if (!strcmp(a, "--version") || !strcmp(a, "-V")) return 2;
        if (!strcmp(a, "--bind")) {
            if (++i >= argc) return snprintf(error, size, "--bind requires an IP address"), -1;
            if (!parse_bind(argv[i], o))
                return snprintf(error, size, "invalid bind IP address: %s", argv[i]), -1;
        } else if (!strcmp(a, "--port")) {
            if (++i >= argc) return snprintf(error, size, "--port requires a TCP port"), -1;
            const char *t = argv[i][0] == '+' ? argv[i] + 1 : argv[i];
            uint64_t v;
            if (!omt_parse_u64(t, strlen(t), 65535, &v))
                return snprintf(error, size, "invalid TCP port: %s", argv[i]), -1;
            if (v == 0) return snprintf(error, size, "TCP port must be between 1 and 65535"), -1;
            o->port = (int)v;
        } else if (!strcmp(a, "--frame-rate")) {
            if (++i >= argc)
                return snprintf(error, size, "--frame-rate requires whole frames per second"), -1;
            const char *t = argv[i][0] == '+' ? argv[i] + 1 : argv[i];
            uint64_t v;
            if (!omt_parse_u64(t, strlen(t), UINT32_MAX, &v))
                return snprintf(error, size, "invalid frame rate: %s", argv[i]), -1;
            if (v == 0 || v > MAX_FRAME_RATE)
                return snprintf(error, size, "frame rate must be between 1 and %u", MAX_FRAME_RATE),
                       -1;
            o->frame_rate = (uint32_t)v;
        } else {
            return snprintf(error, size, "unknown argument: %s", a), -1;
        }
    }
    return 0;
}

static void set_port(sender_options *o, uint16_t port) {
    if (o->bind.ss_family == AF_INET6)
        ((struct sockaddr_in6 *)&o->bind)->sin6_port = htons(port);
    else
        ((struct sockaddr_in *)&o->bind)->sin_port = htons(port);
}

static int listen_on(sender_options *o, char *error, size_t size) {
    int first = o->port ? o->port : DEFAULT_PORT_FIRST;
    int last = o->port ? o->port : DEFAULT_PORT_LAST;
    int last_error = EADDRINUSE;
    for (int port = first; port <= last; port++) {
        int fd = socket(o->bind.ss_family, SOCK_STREAM | SOCK_CLOEXEC, 0);
        if (fd < 0) {
            snprintf(error, size, "unable to listen: %s (os error %d)", strerror(errno), errno);
            return -1;
        }
        int one = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
        set_port(o, (uint16_t)port);
        if (bind(fd, (struct sockaddr *)&o->bind, o->bind_len) == 0 && listen(fd, 128) == 0)
            return fd;
        last_error = errno;
        close(fd);
        if (last_error != EADDRINUSE || o->port) break;
    }
    snprintf(error, size, "unable to listen: %s (os error %d)", strerror(last_error), last_error);
    return -1;
}

static void fixed_header(omt_buf *b, omt_frame_type type, size_t data_length) {
    uint8_t h[OMT_HEADER_SIZE] = {1, (uint8_t)type};
    omt_put_le32(h + 12, (uint32_t)data_length);
    omt_buf_append(b, h, sizeof(h));
}

static void put_i32(omt_buf *b, int32_t v) {
    uint8_t p[4];
    omt_put_le32(p, (uint32_t)v);
    omt_buf_append(b, p, 4);
}

static void put_f32(omt_buf *b, float v) {
    uint32_t bits;
    memcpy(&bits, &v, 4);
    put_i32(b, (int32_t)bits);
}

static void video_frame(omt_buf *b, const uint8_t *vmx, size_t len, uint32_t frame_rate) {
    fixed_header(b, OMT_FRAME_VIDEO, OMT_VIDEO_HEADER_SIZE + len);
    put_i32(b, OMT_CODEC_VMX1);
    put_i32(b, 1920);
    put_i32(b, 1080);
    put_i32(b, (int32_t)frame_rate);
    put_i32(b, 1);
    put_f32(b, 16.0f / 9.0f);
    put_i32(b, 0);
    put_i32(b, 709);
    omt_buf_append(b, vmx, len);
}

/* A modest, deterministic 480 Hz tone. Its eight cycles per 800-sample block
 * repeat without a boundary click. FPA1 is planar: each channel's complete
 * block precedes the next channel. */
static void audio_frame(omt_buf *b) {
    size_t payload = AUDIO_SAMPLES * 2 * sizeof(float);
    fixed_header(b, OMT_FRAME_AUDIO, OMT_AUDIO_HEADER_SIZE + payload);
    put_i32(b, OMT_CODEC_FPA1);
    put_i32(b, AUDIO_SAMPLE_RATE);
    put_i32(b, AUDIO_SAMPLES);
    put_i32(b, 2);
    put_i32(b, 3);
    put_i32(b, 0);
    const float step = 480.0f * 6.2831855f / 48000.0f;
    for (int channel = 0; channel < 2; channel++) {
        float phase = 0.0f;
        for (int i = 0; i < AUDIO_SAMPLES; i++) {
            put_f32(b, channel == 0 ? sinf(phase) * 0.1f : cosf(phase) * 0.1f);
            phase += step;
        }
    }
}

static bool read_exact(int fd, uint8_t *p, size_t len) {
    while (len) {
        ssize_t n = recv(fd, p, len, 0);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            return false;
        }
        p += n;
        len -= (size_t)n;
    }
    return true;
}

static bool read_subscription(int fd, subscription *out, char *error, size_t size) {
    struct timeval tv = {SUBSCRIPTION_TIMEOUT_S, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    for (int i = 0; i < 4; i++) {
        uint8_t fixed[OMT_HEADER_SIZE];
        if (!read_exact(fd, fixed, sizeof(fixed))) {
            snprintf(error, size, "failed to fill whole buffer");
            return false;
        }
        omt_frame_header h;
        omt_proto_error pe;
        if (!omt_parse_frame_header(fixed, sizeof(fixed), &h, &pe)) {
            omt_proto_error_format(&pe, error, size);
            return false;
        }
        if (h.frame_type != OMT_FRAME_METADATA) {
            snprintf(error, size, "expected an OMT metadata subscription");
            return false;
        }
        if (h.data_length == 0 || h.data_length > OMT_METADATA_MAX_SIZE) {
            snprintf(error, size, "invalid OMT subscription length");
            return false;
        }
        char *xml = malloc(h.data_length + 1);
        if (!xml || !read_exact(fd, (uint8_t *)xml, h.data_length)) {
            free(xml);
            snprintf(error, size, "failed to fill whole buffer");
            return false;
        }
        xml[h.data_length] = 0;
        bool video = strstr(xml, "Video=\"true\"") != NULL;
        bool audio = strstr(xml, "Audio=\"true\"") != NULL;
        free(xml);
        if (video) {
            *out = SUB_VIDEO;
            return true;
        }
        if (audio) {
            *out = SUB_AUDIO;
            return true;
        }
    }
    snprintf(error, size, "receiver did not request video or audio");
    return false;
}

typedef struct {
    int fd;
    char peer[96];
    uint32_t frame_rate;
} client_arg;

static bool write_all(int fd, const uint8_t *p, size_t len) {
    while (len) {
        ssize_t n = send(fd, p, len, MSG_NOSIGNAL);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            return false;
        }
        p += n;
        len -= (size_t)n;
    }
    return true;
}

static void stream_frames(int fd, subscription sub, uint32_t frame_rate, char *error, size_t size) {
    struct timeval tv = {WRITE_TIMEOUT_S, 0};
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    omt_buf frames[2];
    size_t count;
    omt_buf_init(&frames[0], 32u * 1024 * 1024);
    omt_buf_init(&frames[1], 32u * 1024 * 1024);
    if (sub == SUB_VIDEO) {
        video_frame(&frames[0], omt_gradient_vmx, omt_gradient_vmx_size, frame_rate);
        video_frame(&frames[1], omt_flat_vmx, omt_flat_vmx_size, frame_rate);
        count = 2;
    } else {
        audio_frame(&frames[0]);
        count = 1;
    }
    /* Audio keeps its own cadence: each block is a fixed number of samples at
     * a fixed rate, so pacing it off the video rate would distort the sound. */
    uint64_t rate = sub == SUB_VIDEO ? frame_rate : AUDIO_BLOCKS_PER_SECOND;
    double started = omt_now_seconds();
    for (uint64_t sequence = 0;; sequence++) {
        uint64_t timestamp = sequence * OMT_TICKS_PER_SECOND / rate;
        omt_buf *frame = &frames[sequence % count];
        omt_put_le64(frame->data + 2, timestamp);
        if (!write_all(fd, frame->data, frame->len)) {
            snprintf(error, size, "%s (os error %d)", strerror(errno), errno);
            break;
        }
        double due = started + (double)(sequence + 1) / (double)rate;
        double delay = due - omt_now_seconds();
        if (delay > 0) {
            struct timespec ts = {(time_t)delay, (long)((delay - (double)(time_t)delay) * 1e9)};
            nanosleep(&ts, NULL);
        }
    }
    omt_buf_free(&frames[0]);
    omt_buf_free(&frames[1]);
}

static void *client_main(void *raw) {
    client_arg *arg = raw;
    char error[256] = "";
    subscription sub;
    if (read_subscription(arg->fd, &sub, error, sizeof(error))) {
        fprintf(stderr, "%s subscribed to %s\n", arg->peer, sub == SUB_VIDEO ? "Video" : "Audio");
        stream_frames(arg->fd, sub, arg->frame_rate, error, sizeof(error));
    }
    fprintf(stderr, "%s disconnected: %s\n", arg->peer, error);
    close(arg->fd);
    free(arg);
    atomic_fetch_sub(&clients, 1);
    return NULL;
}

static void format_address(const struct sockaddr_storage *a, char *out, size_t size) {
    char host[INET6_ADDRSTRLEN] = "?";
    uint16_t port = 0;
    if (a->ss_family == AF_INET6) {
        const struct sockaddr_in6 *v6 = (const struct sockaddr_in6 *)a;
        inet_ntop(AF_INET6, &v6->sin6_addr, host, sizeof(host));
        port = ntohs(v6->sin6_port);
        snprintf(out, size, "[%s]:%u", host, port);
    } else {
        const struct sockaddr_in *v4 = (const struct sockaddr_in *)a;
        inet_ntop(AF_INET, &v4->sin_addr, host, sizeof(host));
        port = ntohs(v4->sin_port);
        snprintf(out, size, "%s:%u", host, port);
    }
}

int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    sender_options o;
    char error[256];
    int r = parse_options(argc, argv, &o, error, sizeof(error));
    if (r == 1) {
        usage();
        return 0;
    }
    if (r == 2) {
        /* The bare semantic version, as the Rust sender printed it. */
        printf("omt-test-sender %s\n", OMT_VERSION[0] == 'v' ? &OMT_VERSION[1] : OMT_VERSION);
        return 0;
    }
    if (r < 0) {
        fprintf(stderr, "ERROR: %s\n", error);
        return 1;
    }
    int listener = listen_on(&o, error, sizeof(error));
    if (listener < 0) {
        fprintf(stderr, "ERROR: %s\n", error);
        return 1;
    }
    struct sockaddr_storage local;
    socklen_t local_len = sizeof(local);
    getsockname(listener, (struct sockaddr *)&local, &local_len);
    char address[128];
    format_address(&local, address, sizeof(address));
    printf("OMT test sender listening on omt://%s\n", address);
    printf("Video: VMX1 1920x1080p%u; audio: FPA1 stereo 48 kHz\n", o.frame_rate);
    fflush(stdout);
    for (;;) {
        struct sockaddr_storage peer;
        socklen_t peer_len = sizeof(peer);
        int fd = accept4(listener, (struct sockaddr *)&peer, &peer_len, SOCK_CLOEXEC);
        if (fd < 0) {
            if (errno == EINTR || errno == ECONNABORTED) continue;
            fprintf(stderr, "ERROR: sender failed: %s (os error %d)\n", strerror(errno), errno);
            return 1;
        }
        char name[128];
        format_address(&peer, name, sizeof(name));
        if (atomic_fetch_add(&clients, 1) >= MAX_CLIENTS) {
            atomic_fetch_sub(&clients, 1);
            fprintf(stderr, "refused %s: client limit reached\n", name);
            close(fd);
            continue;
        }
        client_arg *arg = calloc(1, sizeof(*arg));
        pthread_t thread;
        pthread_attr_t attr;
        pthread_attr_init(&attr);
        pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
        if (!arg) {
            close(fd);
            atomic_fetch_sub(&clients, 1);
        } else {
            arg->fd = fd;
            arg->frame_rate = o.frame_rate;
            omt_strlcpy(arg->peer, name, sizeof(arg->peer));
            if (pthread_create(&thread, &attr, client_main, arg) != 0) {
                fprintf(stderr, "ERROR: sender failed: unable to start a client thread\n");
                return 1;
            }
        }
        pthread_attr_destroy(&attr);
    }
}
