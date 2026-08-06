// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _GNU_SOURCE
#include "drm_output.h"

#include "vmx_api.h"

#include <dirent.h>
#include <drm.h>
#include <drm_fourcc.h>
#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

#define OMT_DRM_NAME_CAPACITY 256u

typedef struct {
    uint32_t handle;
    uint32_t pitch;
    uint64_t size;
    uint32_t framebuffer;
    uint8_t *mapping;
} drm_buffer;

struct omt_drm_output {
    omt_connector connector;
    int fd;
    uint32_t crtc_id;
    drmModeModeInfo mode;
    drm_buffer buffers[3];
    size_t front;
    bool configured;
    bool flip_complete;
    int width;
    int height;
    int frame_rate_n;
    int frame_rate_d;
    int color_space;
    VMX_INSTANCE *codec;
    char error[OMT_ERROR_CAPACITY];
};

static int drm_ioctl(int fd, unsigned long request, void *argument)
{
#if defined(__GLIBC__)
    return ioctl(fd, request, argument);
#else
    return ioctl(fd, (int)request, argument);
#endif
}

static bool read_line(const char *path, char *output, size_t capacity)
{
    int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return false;
    ssize_t count;
    do count = read(fd, output, capacity - 1u); while (count < 0 && errno == EINTR);
    (void)close(fd);
    if (count <= 0) return false;
    size_t length = (size_t)count;
    while (length > 0u && (output[length - 1u] == '\r' || output[length - 1u] == '\n' ||
                           output[length - 1u] == ' ')) length--;
    output[length] = '\0';
    return true;
}

static bool append_path(char *output, size_t capacity, const char *base, const char *suffix)
{
    size_t base_length = strlen(base);
    size_t suffix_length = strlen(suffix);
    if (base_length >= capacity || suffix_length >= capacity - base_length) return false;
    memcpy(output, base, base_length);
    memcpy(output + base_length, suffix, suffix_length + 1u);
    return true;
}

static int compare_names(const void *left, const void *right)
{
    return strcmp((const char *)left, (const char *)right);
}

static bool connector_named(const char *name, omt_connector *connector)
{
    DIR *directory = opendir("/sys/class/drm");
    if (directory == NULL) return false;
    char matches[64][OMT_DRM_NAME_CAPACITY];
    size_t match_count = 0u;
    while (true) {
        struct dirent *entry = readdir(directory);
        if (entry == NULL) break;
        const char *entry_name = entry->d_name;
        size_t entry_length = strlen(entry_name);
        size_t name_length = strlen(name);
        if (entry_length > name_length + 5u && strncmp(entry_name, "card", 4u) == 0 &&
            entry_name[entry_length - name_length - 1u] == '-' &&
            strcmp(entry_name + entry_length - name_length, name) == 0 &&
            match_count < sizeof(matches) / sizeof(matches[0])) {
            (void)omt_copy_string(matches[match_count++], sizeof(matches[0]), entry_name);
        }
    }
    (void)closedir(directory);
    qsort(matches, match_count, sizeof(matches[0]), compare_names);
    for (size_t index = 0u; index < match_count; ++index) {
        char sysfs[OMT_PATH_CAPACITY];
        char status_path[OMT_PATH_CAPACITY];
        char id_path[OMT_PATH_CAPACITY];
        char status[32];
        char id_text[32];
        (void)snprintf(sysfs, sizeof(sysfs), "/sys/class/drm/%s", matches[index]);
        if (!append_path(status_path, sizeof(status_path), sysfs, "/status") ||
            !append_path(id_path, sizeof(id_path), sysfs, "/connector_id") ||
            !read_line(status_path, status, sizeof(status)) || strcmp(status, "connected") != 0 ||
            !read_line(id_path, id_text, sizeof(id_text))) continue;
        char *end = NULL;
        errno = 0;
        unsigned long id = strtoul(id_text, &end, 10);
        if (errno != 0 || end == id_text || *end != '\0' || id == 0ul || id > UINT32_MAX) continue;
        size_t card_length = strlen(matches[index]) - strlen(name) - 1u;
        char card[OMT_DRM_NAME_CAPACITY];
        memcpy(card, matches[index], card_length);
        card[card_length] = '\0';
        char device[OMT_PATH_CAPACITY];
        (void)snprintf(device, sizeof(device), "/dev/dri/%s", card);
        if (access(device, F_OK) != 0) continue;
        memset(connector, 0, sizeof(*connector));
        if (omt_copy_string(connector->name, sizeof(connector->name), name) &&
            omt_copy_string(connector->device_path, sizeof(connector->device_path), device) &&
            omt_copy_string(connector->sysfs_path, sizeof(connector->sysfs_path), sysfs) &&
            omt_copy_string(connector->alsa_device, sizeof(connector->alsa_device),
                strcmp(name, "HDMI-A-1") == 0 ? "plughw:CARD=vc4hdmi0,DEV=0" :
                                                "plughw:CARD=vc4hdmi1,DEV=0")) {
            connector->connector_id = (uint32_t)id;
            return true;
        }
    }
    return false;
}

bool omt_find_connector(const char *preference, omt_connector *connector)
{
    if (strcmp(preference, "auto") != 0) return connector_named(preference, connector);
    return connector_named("HDMI-A-1", connector) || connector_named("HDMI-A-2", connector);
}

bool omt_connector_is_connected(const omt_connector *connector)
{
    char status_path[OMT_PATH_CAPACITY];
    char id_path[OMT_PATH_CAPACITY];
    char status[32];
    char id[32];
    char expected[32];
    if (!append_path(status_path, sizeof(status_path), connector->sysfs_path, "/status") ||
        !append_path(id_path, sizeof(id_path), connector->sysfs_path, "/connector_id")) return false;
    (void)snprintf(expected, sizeof(expected), "%u", connector->connector_id);
    return read_line(status_path, status, sizeof(status)) && strcmp(status, "connected") == 0 &&
           read_line(id_path, id, sizeof(id)) && strcmp(id, expected) == 0;
}

static void destroy_buffer(omt_drm_output *output, drm_buffer *buffer)
{
    if (buffer->mapping != NULL) (void)munmap(buffer->mapping, (size_t)buffer->size);
    if (output->fd >= 0 && buffer->framebuffer != 0u) (void)drmModeRmFB(output->fd, buffer->framebuffer);
    if (output->fd >= 0 && buffer->handle != 0u) {
        struct drm_mode_destroy_dumb destroy;
        memset(&destroy, 0, sizeof(destroy));
        destroy.handle = buffer->handle;
        (void)drm_ioctl(output->fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy);
    }
    memset(buffer, 0, sizeof(*buffer));
}

omt_drm_output *omt_drm_output_create(const omt_connector *connector)
{
    omt_drm_output *output = calloc(1u, sizeof(*output));
    if (output == NULL) return NULL;
    output->fd = -1;
    output->connector = *connector;
    output->fd = open(connector->device_path, O_RDWR | O_CLOEXEC | O_NOFOLLOW);
    if (output->fd < 0) {
        (void)snprintf(output->error, sizeof(output->error), "Failed to open DRM device: %s", strerror(errno));
        return output;
    }
    uint64_t dumb = 0u;
    if (drmGetCap(output->fd, DRM_CAP_DUMB_BUFFER, &dumb) != 0 || dumb == 0u) {
        omt_set_error(output->error, sizeof(output->error), "DRM device does not support dumb buffers");
        (void)close(output->fd);
        output->fd = -1;
    }
    return output;
}

void omt_drm_output_destroy(omt_drm_output *output)
{
    if (output == NULL) return;
    if (output->codec != NULL) VMX_Destroy(output->codec);
    for (size_t i = 0u; i < 3u; ++i) destroy_buffer(output, &output->buffers[i]);
    if (output->fd >= 0) (void)close(output->fd);
    free(output);
}

bool omt_drm_output_ready(const omt_drm_output *output) { return output != NULL && output->fd >= 0; }
const char *omt_drm_output_error(const omt_drm_output *output) { return output == NULL ? "Unable to allocate DRM output" : output->error; }

static bool create_buffer(omt_drm_output *output, drm_buffer *buffer)
{
    struct drm_mode_create_dumb create;
    memset(&create, 0, sizeof(create));
    create.width = output->mode.hdisplay;
    create.height = output->mode.vdisplay;
    create.bpp = 32u;
    if (drm_ioctl(output->fd, DRM_IOCTL_MODE_CREATE_DUMB, &create) != 0) {
        (void)snprintf(output->error, sizeof(output->error), "Unable to create DRM buffer: %s", strerror(errno));
        return false;
    }
    buffer->handle = create.handle; buffer->pitch = create.pitch; buffer->size = create.size;
    uint32_t handles[4] = {buffer->handle, 0u, 0u, 0u};
    uint32_t pitches[4] = {buffer->pitch, 0u, 0u, 0u};
    uint32_t offsets[4] = {0u, 0u, 0u, 0u};
    if (drmModeAddFB2(output->fd, create.width, create.height, DRM_FORMAT_XRGB8888,
                      handles, pitches, offsets, &buffer->framebuffer, 0u) != 0) {
        (void)snprintf(output->error, sizeof(output->error), "Unable to register DRM framebuffer: %s", strerror(errno));
        destroy_buffer(output, buffer); return false;
    }
    struct drm_mode_map_dumb mapping;
    memset(&mapping, 0, sizeof(mapping));
    mapping.handle = buffer->handle;
    if (drm_ioctl(output->fd, DRM_IOCTL_MODE_MAP_DUMB, &mapping) != 0 ||
        mapping.offset > (uint64_t)INT64_MAX) {
        omt_set_error(output->error, sizeof(output->error), "Unable to map DRM buffer");
        destroy_buffer(output, buffer); return false;
    }
    void *address = mmap(NULL, (size_t)buffer->size, PROT_READ | PROT_WRITE, MAP_SHARED,
                         output->fd, (off_t)mapping.offset);
    if (address == MAP_FAILED) {
        (void)snprintf(output->error, sizeof(output->error), "Unable to map DRM buffer memory: %s", strerror(errno));
        destroy_buffer(output, buffer); return false;
    }
    buffer->mapping = address;
    memset(buffer->mapping, 0, (size_t)buffer->size);
    return true;
}

static double refresh_rate(const drmModeModeInfo *mode)
{
    if (mode->htotal == 0u || mode->vtotal == 0u) return 0.0;
    return (double)mode->clock * 1000.0 / ((double)mode->htotal * (double)mode->vtotal);
}

static omt_present_outcome configure(omt_drm_output *output, const omt_video_header *header)
{
    drmModeConnector *connector = drmModeGetConnector(output->fd, output->connector.connector_id);
    if (connector == NULL || connector->connection != DRM_MODE_CONNECTED || connector->encoder_id == 0u) {
        omt_set_error(output->error, sizeof(output->error), "Selected HDMI connector is unavailable");
        if (connector != NULL) drmModeFreeConnector(connector);
        return OMT_PRESENT_FAILED;
    }
    drmModeEncoder *encoder = drmModeGetEncoder(output->fd, connector->encoder_id);
    if (encoder == NULL || encoder->crtc_id == 0u) {
        omt_set_error(output->error, sizeof(output->error), "Selected HDMI encoder is unavailable");
        if (encoder != NULL) drmModeFreeEncoder(encoder);
        drmModeFreeConnector(connector);
        return OMT_PRESENT_FAILED;
    }
    double requested = (double)header->frame_rate_n / (double)header->frame_rate_d;
    const drmModeModeInfo *selected = NULL;
    for (int pass = 0; pass < 3 && selected == NULL; ++pass) {
        for (int index = 0; index < connector->count_modes; ++index) {
            const drmModeModeInfo *candidate = &connector->modes[index];
            if (candidate->hdisplay != header->width || candidate->vdisplay != header->height ||
                (candidate->flags & DRM_MODE_FLAG_INTERLACE) != 0u) continue;
            double expected = pass == 0 ? requested : pass == 1 ? round(requested) : 60.0;
            if (fabs(refresh_rate(candidate) - expected) < 0.02) { selected = candidate; break; }
        }
    }
    if (selected == NULL) {
        omt_set_error(output->error, sizeof(output->error), "Display has no mode for the OMT video format");
        drmModeFreeEncoder(encoder); drmModeFreeConnector(connector);
        return OMT_PRESENT_UNSUPPORTED_FORMAT;
    }
    for (size_t i = 0u; i < 3u; ++i) destroy_buffer(output, &output->buffers[i]);
    if (output->codec != NULL) { VMX_Destroy(output->codec); output->codec = NULL; }
    output->mode = *selected;
    output->crtc_id = encoder->crtc_id;
    drmModeFreeEncoder(encoder); drmModeFreeConnector(connector);
    for (size_t i = 0u; i < 3u; ++i) if (!create_buffer(output, &output->buffers[i])) return OMT_PRESENT_FAILED;
    uint32_t connector_id = output->connector.connector_id;
    if (drmModeSetCrtc(output->fd, output->crtc_id, output->buffers[0].framebuffer,
                       0u, 0u, &connector_id, 1, &output->mode) != 0) {
        (void)snprintf(output->error, sizeof(output->error), "Unable to set DRM mode: %s", strerror(errno));
        return OMT_PRESENT_FAILED;
    }
    VMX_SIZE dimensions = {header->width, header->height};
    output->codec = VMX_Create(dimensions, VMX_PROFILE_OMT_SQ, (VMX_COLORSPACE)header->color_space);
    if (output->codec == NULL) {
        omt_set_error(output->error, sizeof(output->error), "Unable to create VMX decoder");
        return OMT_PRESENT_FAILED;
    }
    output->width = header->width; output->height = header->height;
    output->frame_rate_n = header->frame_rate_n; output->frame_rate_d = header->frame_rate_d;
    output->color_space = header->color_space; output->front = 0u; output->configured = true;
    return OMT_PRESENTED;
}

static void page_flip(int fd, unsigned int sequence, unsigned int seconds,
                      unsigned int microseconds, void *data)
{
    (void)fd; (void)sequence; (void)seconds; (void)microseconds;
    omt_drm_output *output = data;
    if (output != NULL) output->flip_complete = true;
}

static bool wait_for_flip(omt_drm_output *output)
{
    drmEventContext context;
    memset(&context, 0, sizeof(context));
    context.version = DRM_EVENT_CONTEXT_VERSION;
    context.page_flip_handler = page_flip;
    omt_deadline deadline = omt_deadline_after_ms(500);
    while (!output->flip_complete && omt_remaining_milliseconds(deadline) > 0) {
        struct pollfd descriptor = {output->fd, POLLIN, 0};
        int result = poll(&descriptor, 1, omt_remaining_milliseconds(deadline));
        if (result > 0 && (descriptor.revents & POLLIN) != 0) {
            if (drmHandleEvent(output->fd, &context) != 0) {
                omt_set_error(output->error, sizeof(output->error), "Unable to handle DRM page-flip event");
                return false;
            }
        } else if (result < 0 && errno != EINTR) {
            (void)snprintf(output->error, sizeof(output->error), "Unable to wait for DRM page flip: %s", strerror(errno));
            return false;
        }
    }
    if (!output->flip_complete) {
        omt_set_error(output->error, sizeof(output->error), "DRM page flip timed out");
        return false;
    }
    return true;
}

omt_present_outcome omt_drm_output_present(omt_drm_output *output, omt_frame *frame)
{
    if (frame->header.type != OMT_FRAME_VIDEO || frame->video.codec != OMT_CODEC_VMX1) {
        omt_set_error(output->error, sizeof(output->error), "Unsupported video frame");
        return OMT_PRESENT_UNSUPPORTED_FORMAT;
    }
    if (!output->configured || output->width != frame->video.width || output->height != frame->video.height ||
        output->frame_rate_n != frame->video.frame_rate_n || output->frame_rate_d != frame->video.frame_rate_d ||
        output->color_space != frame->video.color_space) {
        omt_present_outcome outcome = configure(output, &frame->video);
        if (outcome != OMT_PRESENTED) return outcome;
    }
    size_t next = (output->front + 1u) % 3u;
    size_t payload_offset = OMT_WIRE_VIDEO_HEADER_SIZE;
    if ((size_t)frame->header.data_length < payload_offset + frame->header.metadata_length) {
        omt_set_error(output->error, sizeof(output->error), "Truncated VMX frame");
        return OMT_PRESENT_FAILED;
    }
    size_t compressed = (size_t)frame->header.data_length - payload_offset - frame->header.metadata_length;
    if (compressed > INT32_MAX) {
        omt_set_error(output->error, sizeof(output->error), "VMX frame is too large");
        return OMT_PRESENT_FAILED;
    }
    if (VMX_LoadFrom(output->codec, frame->payload + payload_offset, (int)compressed) != VMX_ERR_OK ||
        VMX_DecodeBGRX(output->codec, output->buffers[next].mapping,
                       (int)output->buffers[next].pitch) != VMX_ERR_OK) {
        omt_set_error(output->error, sizeof(output->error), "VMX decoder rejected the frame");
        return OMT_PRESENT_FAILED;
    }
    output->flip_complete = false;
    if (drmModePageFlip(output->fd, output->crtc_id, output->buffers[next].framebuffer,
                        DRM_MODE_PAGE_FLIP_EVENT, output) != 0) {
        (void)snprintf(output->error, sizeof(output->error), "Unable to queue DRM page flip: %s", strerror(errno));
        return OMT_PRESENT_FAILED;
    }
    if (!wait_for_flip(output)) return OMT_PRESENT_FAILED;
    output->front = next;
    return OMT_PRESENTED;
}
