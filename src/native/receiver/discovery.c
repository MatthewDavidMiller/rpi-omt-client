// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _GNU_SOURCE
#include "discovery.h"

#include "omt_channel.h"

#include <avahi-client/client.h>
#include <avahi-client/lookup.h>
#include <avahi-common/address.h>
#include <avahi-common/simple-watch.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

#define SETTINGS_MAX_BYTES (64u * 1024u)

typedef struct {
    AvahiSimplePoll *poll;
    AvahiClient *client;
    omt_source *sources;
    size_t count;
    size_t capacity;
    AvahiServiceResolver *resolvers[OMT_MAX_SOURCES];
    size_t resolver_count;
} discovery_context;

static int compare_sources(const void *left, const void *right)
{
    return strcmp(((const omt_source *)left)->name, ((const omt_source *)right)->name);
}

static char *bounded_file(const char *path, size_t limit)
{
    int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return NULL;
    struct stat info;
    if (fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) || info.st_size < 0 ||
        (uintmax_t)info.st_size > limit) {
        (void)close(fd); return NULL;
    }
    size_t length = (size_t)info.st_size;
    char *text = malloc(length + 1u);
    if (text == NULL) { (void)close(fd); return NULL; }
    size_t offset = 0u;
    while (offset < length) {
        ssize_t count = read(fd, text + offset, length - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { free(text); (void)close(fd); return NULL; }
        offset += (size_t)count;
    }
    text[length] = '\0';
    (void)close(fd);
    return text;
}

static bool xml_text(const char *xml, const char *tag, char *output, size_t capacity)
{
    char open[64];
    char close[64];
    if (snprintf(open, sizeof(open), "<%s>", tag) < 0 ||
        snprintf(close, sizeof(close), "</%s>", tag) < 0) return false;
    const char *begin = strstr(xml, open);
    if (begin == NULL || strstr(begin + strlen(open), open) != NULL) return false;
    begin += strlen(open);
    const char *end = strstr(begin, close);
    size_t length = end == NULL ? capacity : (size_t)(end - begin);
    if (end == NULL || length >= capacity) return false;
    memcpy(output, begin, length);
    output[length] = '\0';
    return true;
}

static bool decode_xml(const char *input, char *output, size_t capacity)
{
    size_t used = 0u;
    while (*input != '\0') {
        char value = *input++;
        if (value == '<' || value == '>') return false;
        if (value == '&') {
            struct entity { const char *text; char value; };
            static const struct entity entities[] = {
                {"amp;", '&'}, {"lt;", '<'}, {"gt;", '>'}, {"quot;", '"'}, {"apos;", '\''}
            };
            bool found = false;
            for (size_t i = 0u; i < sizeof(entities) / sizeof(entities[0]); ++i) {
                size_t length = strlen(entities[i].text);
                if (strncmp(input, entities[i].text, length) == 0) {
                    value = entities[i].value;
                    input += length;
                    found = true;
                    break;
                }
            }
            if (!found) return false;
        }
        if (used + 1u >= capacity) return false;
        output[used++] = value;
    }
    output[used] = '\0';
    return true;
}

static bool endpoint_from_xml(const char *xml, omt_endpoint *endpoint)
{
    char encoded[OMT_ENDPOINT_HOST_CAPACITY] = {0};
    char address[OMT_ENDPOINT_HOST_CAPACITY] = {0};
    char port_text[16] = {0};
    if (!xml_text(xml, "IPAddress", encoded, sizeof(encoded)) ||
        !decode_xml(encoded, address, sizeof(address)) ||
        !xml_text(xml, "Port", port_text, sizeof(port_text))) return false;
    char *end = NULL;
    errno = 0;
    unsigned long port = strtoul(port_text, &end, 10);
    if (errno != 0 || end == port_text || *end != '\0' || port == 0ul || port > 65535ul) return false;
    char candidate[OMT_ENDPOINT_HOST_CAPACITY + 32u];
    int count = strchr(address, ':') == NULL
        ? snprintf(candidate, sizeof(candidate), "omt://%s:%lu", address, port)
        : snprintf(candidate, sizeof(candidate), "omt://[%s]:%lu", address, port);
    omt_direct_target parsed;
    if (count < 0 || (size_t)count >= sizeof(candidate) ||
        !omt_parse_direct_target(candidate, &parsed)) return false;
    return omt_copy_string(endpoint->host, sizeof(endpoint->host), parsed.host) &&
           (endpoint->port = parsed.port) != 0u;
}

static bool configured_server(char *output, size_t capacity)
{
    const char *storage = getenv("OMT_STORAGE_PATH");
    char path[OMT_PATH_CAPACITY];
    int count = snprintf(path, sizeof(path), "%s/settings.xml",
                         storage == NULL ? "/etc/omt/omt" : storage);
    if (count < 0 || (size_t)count >= sizeof(path)) return false;
    char *document = bounded_file(path, SETTINGS_MAX_BYTES);
    if (document == NULL || strstr(document, "<!") != NULL) { free(document); return false; }
    char *begin = document;
    while (*begin == ' ' || *begin == '\t' || *begin == '\r' || *begin == '\n') begin++;
    char *end = document + strlen(document);
    while (end > begin && (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\r' || end[-1] == '\n')) end--;
    *end = '\0';
    if (strncmp(begin, "<?xml", 5u) == 0) {
        char *declaration_end = strstr(begin, "?>");
        if (declaration_end == NULL) { free(document); return false; }
        begin = declaration_end + 2u;
        while (*begin == ' ' || *begin == '\t' || *begin == '\r' || *begin == '\n') begin++;
    }
    size_t envelope_length = strlen(begin);
    bool settings_start = strncmp(begin, "<Settings", 9u) == 0 &&
        (begin[9] == '>' || begin[9] == ' ' || begin[9] == '\t' ||
         begin[9] == '\r' || begin[9] == '\n');
    if (!settings_start || envelope_length < 11u ||
        strcmp(begin + envelope_length - 11u, "</Settings>") != 0) {
        free(document); return false;
    }
    char encoded[OMT_ENDPOINT_HOST_CAPACITY + 32u];
    bool result = xml_text(document, "DiscoveryServer", encoded, sizeof(encoded)) &&
                  decode_xml(encoded, output, capacity);
    omt_direct_target parsed;
    result = result && omt_parse_direct_target(output, &parsed);
    free(document);
    return result;
}

static bool bus_path(char *output, size_t capacity)
{
    const char *address = getenv("DBUS_SYSTEM_BUS_ADDRESS");
    if (address == NULL) address = "unix:path=/run/dbus/system_bus_socket";
    const char *begin = strstr(address, "unix:path=");
    if (begin == NULL) return false;
    begin += strlen("unix:path=");
    size_t length = strcspn(begin, ",;");
    if (length == 0u || length >= capacity || length >= sizeof(((struct sockaddr_un *)0)->sun_path)) return false;
    memcpy(output, begin, length);
    output[length] = '\0';
    return true;
}

static bool avahi_bus_available(void)
{
    char path[sizeof(((struct sockaddr_un *)0)->sun_path)];
    if (!bus_path(path, sizeof(path))) return false;
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) return false;
    struct sockaddr_un address;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    (void)omt_copy_string(address.sun_path, sizeof(address.sun_path), path);
    bool result = connect(fd, (const struct sockaddr *)&address, sizeof(address)) == 0;
    (void)close(fd);
    return result;
}

static void remove_resolver(discovery_context *context, AvahiServiceResolver *resolver)
{
    for (size_t i = 0u; i < context->resolver_count; ++i) {
        if (context->resolvers[i] == resolver) {
            context->resolvers[i] = context->resolvers[--context->resolver_count];
            return;
        }
    }
}

static void resolve_callback(AvahiServiceResolver *resolver, AvahiIfIndex interface,
    AvahiProtocol protocol, AvahiResolverEvent event, const char *name, const char *type,
    const char *domain, const char *host_name, const AvahiAddress *address, uint16_t port,
    AvahiStringList *txt, AvahiLookupResultFlags flags, void *userdata)
{
    (void)protocol; (void)type; (void)domain; (void)host_name; (void)txt; (void)flags;
    discovery_context *context = userdata;
    if (context != NULL) remove_resolver(context, resolver);
    if (event == AVAHI_RESOLVER_FOUND && context != NULL && name != NULL && address != NULL &&
        port != 0u && context->count < context->capacity && omt_is_valid_source_name_utf8(name)) {
        bool duplicate = false;
        for (size_t i = 0u; i < context->count; ++i) duplicate |= strcmp(context->sources[i].name, name) == 0;
        char host[AVAHI_ADDRESS_STR_MAX + 32u] = {0};
        if (!duplicate && avahi_address_snprint(host, sizeof(host), address) != NULL) {
            if (address->proto == AVAHI_PROTO_INET6 && interface > 0 &&
                address->data.ipv6.address[0] == 0xfeu &&
                (address->data.ipv6.address[1] & 0xc0u) == 0x80u) {
                size_t used = strlen(host);
                (void)snprintf(host + used, sizeof(host) - used, "%%%d", interface);
            }
            omt_source *source = &context->sources[context->count];
            if (omt_copy_string(source->name, sizeof(source->name), name) &&
                omt_copy_string(source->endpoint.host, sizeof(source->endpoint.host), host)) {
                source->endpoint.port = port;
                context->count++;
            }
        }
    }
    if (resolver != NULL) avahi_service_resolver_free(resolver);
}

static void browse_callback(AvahiServiceBrowser *browser, AvahiIfIndex interface,
    AvahiProtocol protocol, AvahiBrowserEvent event, const char *name, const char *type,
    const char *domain, AvahiLookupResultFlags flags, void *userdata)
{
    (void)browser; (void)flags;
    discovery_context *context = userdata;
    if (context == NULL || name == NULL) return;
    if (event == AVAHI_BROWSER_REMOVE) {
        for (size_t i = 0u; i < context->count; ++i) {
            if (strcmp(context->sources[i].name, name) == 0) {
                context->sources[i] = context->sources[--context->count]; break;
            }
        }
    } else if (event == AVAHI_BROWSER_NEW && context->client != NULL && type != NULL &&
               domain != NULL && context->resolver_count < OMT_MAX_SOURCES) {
        AvahiServiceResolver *resolver = avahi_service_resolver_new(
            context->client, interface, protocol, name, type, domain, AVAHI_PROTO_UNSPEC,
            (AvahiLookupFlags)0, resolve_callback, context);
        if (resolver != NULL) context->resolvers[context->resolver_count++] = resolver;
    }
}

static void client_callback(AvahiClient *client, AvahiClientState state, void *userdata)
{
    (void)client;
    discovery_context *context = userdata;
    if (context != NULL && context->poll != NULL && state == AVAHI_CLIENT_FAILURE)
        avahi_simple_poll_quit(context->poll);
}

static size_t server_sources(const char *target, omt_source *sources, size_t capacity, int wait_ms)
{
    omt_direct_target parsed;
    if (!omt_parse_direct_target(target, &parsed)) return 0u;
    omt_endpoint endpoint;
    if (!omt_copy_string(endpoint.host, sizeof(endpoint.host), parsed.host)) return 0u;
    endpoint.port = parsed.port;
    omt_channel *channel = omt_channel_create();
    omt_frame frame;
    omt_frame_init(&frame);
    char error[OMT_ERROR_CAPACITY] = {0};
    omt_deadline deadline = omt_deadline_after_ms(wait_ms);
    if (channel == NULL || !omt_channel_connect(channel, &endpoint, OMT_FRAME_METADATA,
                                                 deadline, error, sizeof(error))) {
        omt_channel_destroy(channel); return 0u;
    }
    size_t count = 0u;
    while (count < capacity && omt_remaining_milliseconds(deadline) > 0 &&
           omt_channel_receive(channel, &frame, deadline, error, sizeof(error))) {
        if (frame.header.type != OMT_FRAME_METADATA || frame.header.data_length == 0u ||
            frame.header.data_length > OMT_WIRE_METADATA_MAX_SIZE ||
            memchr(frame.payload, 0, frame.header.data_length) != NULL) continue;
        char xml[OMT_WIRE_METADATA_MAX_SIZE + 1u];
        memcpy(xml, frame.payload, frame.header.data_length);
        xml[frame.header.data_length] = '\0';
        char encoded[OMT_SOURCE_NAME_CAPACITY * 6u];
        char name[OMT_SOURCE_NAME_CAPACITY];
        if (strstr(xml, "<!") != NULL || !xml_text(xml, "Name", encoded, sizeof(encoded)) ||
            !decode_xml(encoded, name, sizeof(name)) || !omt_is_valid_source_name_utf8(name)) continue;
        size_t existing = count;
        for (size_t i = 0u; i < count; ++i) if (strcmp(sources[i].name, name) == 0) existing = i;
        if (strstr(xml, "<Removed>True</Removed>") != NULL) {
            if (existing < count) sources[existing] = sources[--count];
        } else if (existing == count && endpoint_from_xml(xml, &sources[count].endpoint) &&
                   omt_copy_string(sources[count].name, sizeof(sources[count].name), name)) {
            count++;
        }
    }
    omt_frame_destroy(&frame);
    omt_channel_destroy(channel);
    qsort(sources, count, sizeof(*sources), compare_sources);
    return count;
}

bool omt_discovery_transport_available(void)
{
    char server[OMT_ENDPOINT_HOST_CAPACITY + 32u];
    return configured_server(server, sizeof(server)) || avahi_bus_available();
}

size_t omt_discover_sources(omt_source *sources, size_t capacity, int wait_ms)
{
    if (capacity > OMT_MAX_SOURCES) capacity = OMT_MAX_SOURCES;
    char server[OMT_ENDPOINT_HOST_CAPACITY + 32u];
    if (configured_server(server, sizeof(server))) return server_sources(server, sources, capacity, wait_ms);
    if (!avahi_bus_available()) return 0u;
    discovery_context context;
    memset(&context, 0, sizeof(context));
    context.sources = sources;
    context.capacity = capacity;
    context.poll = avahi_simple_poll_new();
    if (context.poll == NULL) return 0u;
    int error = 0;
    context.client = avahi_client_new(avahi_simple_poll_get(context.poll), AVAHI_CLIENT_NO_FAIL,
                                      client_callback, &context, &error);
    AvahiServiceBrowser *browser = context.client == NULL ? NULL :
        avahi_service_browser_new(context.client, AVAHI_IF_UNSPEC, AVAHI_PROTO_UNSPEC,
                                  "_omt._tcp", NULL, (AvahiLookupFlags)0, browse_callback, &context);
    omt_deadline deadline = omt_deadline_after_ms(wait_ms);
    while (browser != NULL && omt_remaining_milliseconds(deadline) > 0) {
        int remaining = omt_remaining_milliseconds(deadline);
        if (avahi_simple_poll_iterate(context.poll, remaining < 100 ? remaining : 100) != 0) break;
    }
    if (browser != NULL) avahi_service_browser_free(browser);
    for (size_t i = 0u; i < context.resolver_count; ++i) avahi_service_resolver_free(context.resolvers[i]);
    if (context.client != NULL) avahi_client_free(context.client);
    avahi_simple_poll_free(context.poll);
    qsort(sources, context.count, sizeof(*sources), compare_sources);
    return context.count;
}

bool omt_resolve_target(const char *target, int wait_ms, omt_endpoint *endpoint)
{
    omt_direct_target direct;
    if (omt_parse_direct_target(target, &direct)) {
        endpoint->port = direct.port;
        return omt_copy_string(endpoint->host, sizeof(endpoint->host), direct.host);
    }
    if (strncmp(target, "omt://", 6u) == 0 || !omt_is_valid_source_name_utf8(target)) return false;
    omt_source sources[OMT_MAX_SOURCES];
    size_t count = omt_discover_sources(sources, OMT_MAX_SOURCES, wait_ms);
    for (size_t i = 0u; i < count; ++i) {
        if (strcmp(sources[i].name, target) == 0) {
            *endpoint = sources[i].endpoint;
            return true;
        }
    }
    return false;
}
