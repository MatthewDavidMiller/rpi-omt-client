// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "discovery.hpp"

#include "omt_channel.hpp"

#include "omt/omt_wire.h"

#include <avahi-client/client.h>
#include <avahi-client/lookup.h>
#include <avahi-common/address.h>
#include <avahi-common/error.h>
#include <avahi-common/simple-watch.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <limits>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace omt::native {
namespace {

constexpr std::size_t max_sources = 256;
constexpr std::size_t max_settings_bytes = 64U * 1024U;

struct DiscoveryContext {
    AvahiSimplePoll* poll{};
    AvahiClient* client{};
    std::vector<Source> sources;
    std::vector<AvahiServiceResolver*> resolvers;
};

void resolve_callback(
    AvahiServiceResolver* resolver,
    AvahiIfIndex interface,
    AvahiProtocol,
    AvahiResolverEvent event,
    const char* name,
    const char*,
    const char*,
    const char*,
    const AvahiAddress* address,
    std::uint16_t port,
    AvahiStringList*,
    AvahiLookupResultFlags,
    void* userdata)
{
    auto* context = static_cast<DiscoveryContext*>(userdata);
    if (context != nullptr) {
        std::erase(context->resolvers, resolver);
    }
    if (event == AVAHI_RESOLVER_FOUND && context != nullptr && name != nullptr && address != nullptr &&
        port != 0u && context->sources.size() < max_sources && omt_is_valid_source_name_utf8(name)) {
        std::array<char, AVAHI_ADDRESS_STR_MAX> text{};
        if (avahi_address_snprint(text.data(), text.size(), address) != nullptr) {
            std::string host(text.data());
            if (address->proto == AVAHI_PROTO_INET6 && interface > 0 &&
                address->data.ipv6.address[0] == 0xfeu &&
                (address->data.ipv6.address[1] & 0xc0u) == 0x80u) {
                host += "%" + std::to_string(interface);
            }
            auto duplicate = std::find_if(
                context->sources.begin(), context->sources.end(),
                [name](const Source& source) { return source.name == name; });
            if (duplicate == context->sources.end()) {
                context->sources.push_back(Source{name, Endpoint{std::move(host), port}});
            }
        }
    }
    if (resolver != nullptr) {
        avahi_service_resolver_free(resolver);
    }
}

void browse_callback(
    AvahiServiceBrowser*,
    AvahiIfIndex interface,
    AvahiProtocol protocol,
    AvahiBrowserEvent event,
    const char* name,
    const char* type,
    const char* domain,
    AvahiLookupResultFlags,
    void* userdata)
{
    auto* context = static_cast<DiscoveryContext*>(userdata);
    if (context == nullptr || name == nullptr) {
        return;
    }
    if (event == AVAHI_BROWSER_REMOVE) {
        std::erase_if(context->sources, [name](const Source& source) { return source.name == name; });
        return;
    }
    if (context->client == nullptr || event != AVAHI_BROWSER_NEW || type == nullptr ||
        domain == nullptr || context->resolvers.size() >= max_sources) {
        return;
    }
    AvahiServiceResolver* resolver = avahi_service_resolver_new(
        context->client,
        interface,
        protocol,
        name,
        type,
        domain,
        AVAHI_PROTO_UNSPEC,
        static_cast<AvahiLookupFlags>(0),
        resolve_callback,
        context);
    if (resolver != nullptr) {
        context->resolvers.push_back(resolver);
    }
}

void client_callback(AvahiClient*, AvahiClientState state, void* userdata)
{
    auto* context = static_cast<DiscoveryContext*>(userdata);
    if (context != nullptr && context->poll != nullptr && state == AVAHI_CLIENT_FAILURE) {
        avahi_simple_poll_quit(context->poll);
    }
}

std::string bus_socket_path()
{
    const char* raw = std::getenv("DBUS_SYSTEM_BUS_ADDRESS");
    std::string address = raw == nullptr ? "unix:path=/run/dbus/system_bus_socket" : raw;
    constexpr std::string_view prefix = "unix:path=";
    std::size_t begin = address.find(prefix);
    if (begin == std::string::npos) {
        return {};
    }
    begin += prefix.size();
    std::size_t end = address.find_first_of(",;", begin);
    std::string path = address.substr(begin, end == std::string::npos ? std::string::npos : end - begin);
    return path.size() < sizeof(sockaddr_un::sun_path) ? path : std::string{};
}

std::optional<std::string> xml_text(std::string_view xml, std::string_view tag)
{
    const std::string open = "<" + std::string(tag) + ">";
    const std::string close = "</" + std::string(tag) + ">";
    const std::size_t begin = xml.find(open);
    if (begin == std::string_view::npos || xml.find(open, begin + open.size()) != std::string_view::npos) {
        return std::nullopt;
    }
    const std::size_t content = begin + open.size();
    const std::size_t end = xml.find(close, content);
    if (end == std::string_view::npos) {
        return std::nullopt;
    }
    return std::string(xml.substr(content, end - content));
}

std::optional<std::string> decode_xml_text(std::string_view value)
{
    std::string output;
    output.reserve(value.size());
    for (std::size_t index = 0; index < value.size();) {
        if (value[index] != '&') {
            if (value[index] == '<' || value[index] == '>') {
                return std::nullopt;
            }
            output += value[index++];
            continue;
        }
        constexpr std::array<std::pair<std::string_view, char>, 5> entities{{
            {"&amp;", '&'}, {"&lt;", '<'}, {"&gt;", '>'}, {"&quot;", '"'}, {"&apos;", '\''}}};
        bool found = false;
        for (const auto& [entity, character] : entities) {
            if (value.substr(index).starts_with(entity)) {
                output += character;
                index += entity.size();
                found = true;
                break;
            }
        }
        if (!found) {
            return std::nullopt;
        }
    }
    return output;
}

std::optional<Endpoint> endpoint_from_xml(std::string_view xml)
{
    auto address_text = xml_text(xml, "IPAddress");
    auto port_text = xml_text(xml, "Port");
    if (!address_text.has_value() || !port_text.has_value()) {
        return std::nullopt;
    }
    auto address = decode_xml_text(*address_text);
    if (!address.has_value() || address->empty()) {
        return std::nullopt;
    }
    unsigned long port = 0;
    for (const char character : *port_text) {
        if (character < '0' || character > '9') {
            return std::nullopt;
        }
        port = port * 10UL + static_cast<unsigned long>(character - '0');
        if (port > 65'535UL) {
            return std::nullopt;
        }
    }
    if (port == 0) {
        return std::nullopt;
    }
    const std::string candidate = address->find(':') == std::string::npos
                                      ? "omt://" + *address + ":" + std::to_string(port)
                                      : "omt://[" + *address + "]:" + std::to_string(port);
    omt_direct_target parsed{};
    if (!omt_parse_direct_target(candidate.c_str(), &parsed)) {
        return std::nullopt;
    }
    return Endpoint{parsed.host, parsed.port};
}

std::optional<std::string> configured_server()
{
    const char* storage = std::getenv("OMT_STORAGE_PATH");
    const std::string path = std::string(storage == nullptr ? "/etc/omt/omt" : storage) + "/settings.xml";
    const int descriptor = ::open(path.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (descriptor < 0) {
        return std::nullopt;
    }
    struct stat information {};
    if (::fstat(descriptor, &information) != 0 || !S_ISREG(information.st_mode) || information.st_size < 0 ||
        static_cast<std::uintmax_t>(information.st_size) > max_settings_bytes) {
        (void)::close(descriptor);
        return std::nullopt;
    }
    std::string document(static_cast<std::size_t>(information.st_size), '\0');
    std::size_t offset = 0;
    while (offset < document.size()) {
        const ssize_t count = ::read(descriptor, document.data() + offset, document.size() - offset);
        if (count < 0 && errno == EINTR) {
            continue;
        }
        if (count <= 0) {
            (void)::close(descriptor);
            return std::nullopt;
        }
        offset += static_cast<std::size_t>(count);
    }
    (void)::close(descriptor);
    std::string_view envelope(document);
    const auto trim = [](std::string_view value) {
        const auto first = value.find_first_not_of(" \t\r\n");
        if (first == std::string_view::npos) return std::string_view{};
        const auto last = value.find_last_not_of(" \t\r\n");
        return value.substr(first, last - first + 1u);
    };
    envelope = trim(envelope);
    if (envelope.starts_with("<?xml")) {
        const auto declaration_end = envelope.find("?>");
        if (declaration_end == std::string_view::npos) return std::nullopt;
        envelope = trim(envelope.substr(declaration_end + 2u));
    }
    const bool settings_start = envelope.starts_with("<Settings") && envelope.size() > 9u &&
                                (envelope[9] == '>' || envelope[9] == ' ' ||
                                 envelope[9] == '\t' || envelope[9] == '\r' || envelope[9] == '\n');
    if (document.find("<!") != std::string::npos || !settings_start ||
        !envelope.ends_with("</Settings>")) {
        return std::nullopt;
    }
    auto server = xml_text(document, "DiscoveryServer");
    if (!server.has_value() || server->empty()) {
        return std::nullopt;
    }
    auto decoded = decode_xml_text(*server);
    omt_direct_target target{};
    if (!decoded.has_value() || !omt_parse_direct_target(decoded->c_str(), &target)) {
        return std::nullopt;
    }
    return decoded;
}

std::vector<Source> discover_server_sources(std::string_view target, std::chrono::milliseconds wait)
{
    std::vector<Source> sources;
    omt_direct_target parsed{};
    const std::string owned(target);
    if (!omt_parse_direct_target(owned.c_str(), &parsed)) {
        return sources;
    }
    OmtChannel channel;
    std::string error;
    const Deadline deadline = deadline_after(wait);
    if (!channel.connect(Endpoint{parsed.host, parsed.port}, OMT_FRAME_METADATA, deadline, error)) {
        return sources;
    }
    while (sources.size() < max_sources && remaining_milliseconds(deadline) > 0) {
        Frame frame;
        if (!channel.receive(frame, deadline, error)) {
            break;
        }
        if (frame.header.type != OMT_FRAME_METADATA || frame.payload.empty() ||
            frame.payload.size() > OMT_WIRE_METADATA_MAX_SIZE ||
            std::find(frame.payload.begin(), frame.payload.end(), 0U) != frame.payload.end()) {
            continue;
        }
        const std::string xml(frame.payload.begin(), frame.payload.end());
        if (xml.find("<!") != std::string::npos) {
            continue;
        }
        auto encoded_name = xml_text(xml, "Name");
        if (!encoded_name.has_value()) {
            continue;
        }
        auto name = decode_xml_text(*encoded_name);
        if (!name.has_value() || !omt_is_valid_source_name_utf8(name->c_str())) {
            continue;
        }
        if (xml.find("<Removed>True</Removed>") != std::string::npos) {
            std::erase_if(sources, [&name](const Source& source) { return source.name == *name; });
            continue;
        }
        auto endpoint = endpoint_from_xml(xml);
        if (!endpoint.has_value()) {
            continue;
        }
        const auto duplicate = std::ranges::find_if(sources, [&name](const Source& source) {
            return source.name == *name;
        });
        if (duplicate == sources.end()) {
            sources.push_back(Source{std::move(*name), std::move(*endpoint)});
        }
    }
    std::ranges::sort(sources, {}, &Source::name);
    return sources;
}

} // namespace

bool avahi_bus_available()
{
    std::string path = bus_socket_path();
    if (path.empty()) {
        return false;
    }
    int fd = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return false;
    }
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    std::memcpy(address.sun_path, path.c_str(), path.size() + 1u);
    bool connected = ::connect(fd, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) == 0;
    (void)::close(fd);
    return connected;
}

bool discovery_transport_available()
{
    return configured_server().has_value() || avahi_bus_available();
}

std::vector<Source> discover_sources(std::chrono::milliseconds wait)
{
    if (const auto server = configured_server(); server.has_value()) {
        return discover_server_sources(*server, wait);
    }
    DiscoveryContext context{};
    if (!avahi_bus_available()) {
        return context.sources;
    }
    context.poll = avahi_simple_poll_new();
    if (context.poll == nullptr) {
        return context.sources;
    }
    int error = 0;
    context.client = avahi_client_new(
        avahi_simple_poll_get(context.poll),
        AVAHI_CLIENT_NO_FAIL,
        client_callback,
        &context,
        &error);
    AvahiServiceBrowser* browser = nullptr;
    if (context.client != nullptr) {
        browser = avahi_service_browser_new(
            context.client,
            AVAHI_IF_UNSPEC,
            AVAHI_PROTO_UNSPEC,
            "_omt._tcp",
            nullptr,
            static_cast<AvahiLookupFlags>(0),
            browse_callback,
            &context);
    }
    Deadline deadline = deadline_after(wait);
    while (browser != nullptr && remaining_milliseconds(deadline) > 0) {
        int slice = std::min(100, remaining_milliseconds(deadline));
        if (avahi_simple_poll_iterate(context.poll, slice) != 0) {
            break;
        }
    }
    if (browser != nullptr) {
        avahi_service_browser_free(browser);
    }
    for (AvahiServiceResolver* resolver : context.resolvers) {
        avahi_service_resolver_free(resolver);
    }
    context.resolvers.clear();
    if (context.client != nullptr) {
        avahi_client_free(context.client);
    }
    avahi_simple_poll_free(context.poll);
    std::sort(context.sources.begin(), context.sources.end(), [](const Source& left, const Source& right) {
        return left.name < right.name;
    });
    return context.sources;
}

std::optional<Endpoint> resolve_target(std::string_view target, std::chrono::milliseconds wait)
{
    std::string owned(target);
    omt_direct_target direct{};
    if (omt_parse_direct_target(owned.c_str(), &direct)) {
        return Endpoint{direct.host, direct.port};
    }
    if (owned.starts_with("omt://")) {
        return std::nullopt;
    }
    if (!omt_is_valid_source_name_utf8(owned.c_str())) {
        return std::nullopt;
    }
    std::vector<Source> sources = discover_sources(wait);
    auto result = std::find_if(sources.begin(), sources.end(), [&owned](const Source& source) {
        return source.name == owned;
    });
    if (result == sources.end()) {
        return std::nullopt;
    }
    return result->endpoint;
}

} // namespace omt::native
