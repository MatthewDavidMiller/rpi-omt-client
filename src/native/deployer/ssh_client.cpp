#include "ssh_client.hpp"

#include <libssh2.h>
#include <libssh2_sftp.h>

#include <array>
#include <cerrno>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <mutex>
#include <stdexcept>
#include <system_error>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
using Socket = SOCKET;
constexpr Socket invalid_socket = INVALID_SOCKET;
#else
#include <fcntl.h>
#include <netdb.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>
using Socket = int;
constexpr Socket invalid_socket = -1;
#endif

namespace omt::deployer {
namespace {
constexpr std::size_t max_remote_output = 4U * 1024U * 1024U;
constexpr long connect_timeout_seconds = 15;

void close_socket(const Socket socket) noexcept {
#ifdef _WIN32
    closesocket(socket);
#else
    close(socket);
#endif
}

int connect_socket(const Socket socket, const addrinfo& address) {
#ifdef _WIN32
    return ::connect(socket, address.ai_addr, static_cast<int>(address.ai_addrlen));
#else
    return ::connect(socket, address.ai_addr, static_cast<socklen_t>(address.ai_addrlen));
#endif
}

bool begin_nonblocking(const Socket socket, int& original_flags) noexcept {
#ifdef _WIN32
    (void)original_flags;
    u_long enabled = 1;
    return ioctlsocket(socket, FIONBIO, &enabled) == 0;
#else
    original_flags = fcntl(socket, F_GETFL, 0);
    return original_flags >= 0 && fcntl(socket, F_SETFL, original_flags | O_NONBLOCK) == 0;
#endif
}

bool restore_blocking(const Socket socket, const int original_flags) noexcept {
#ifdef _WIN32
    (void)original_flags;
    u_long disabled = 0;
    return ioctlsocket(socket, FIONBIO, &disabled) == 0;
#else
    return fcntl(socket, F_SETFL, original_flags) == 0;
#endif
}

bool connect_pending() noexcept {
#ifdef _WIN32
    const int error = WSAGetLastError();
    return error == WSAEINPROGRESS || error == WSAEWOULDBLOCK || error == WSAEINVAL;
#else
    return errno == EINPROGRESS || errno == EWOULDBLOCK;
#endif
}

bool connect_with_timeout(const Socket socket, const addrinfo& address) noexcept {
    int original_flags = 0;
    if (!begin_nonblocking(socket, original_flags)) {
        return false;
    }
    const auto finish = [&](const bool connected) {
        return restore_blocking(socket, original_flags) && connected;
    };
    if (connect_socket(socket, address) == 0) {
        return finish(true);
    }
    if (!connect_pending()) {
        return finish(false);
    }

    fd_set write_set;
    fd_set error_set;
    FD_ZERO(&write_set);
    FD_ZERO(&error_set);
    FD_SET(socket, &write_set);
    FD_SET(socket, &error_set);
    timeval timeout{connect_timeout_seconds, 0};
#ifdef _WIN32
    const int ready = ::select(0, nullptr, &write_set, &error_set, &timeout);
#else
    const int ready = ::select(socket + 1, nullptr, &write_set, &error_set, &timeout);
#endif
    if (ready <= 0) {
        return finish(false);
    }
    int socket_error = 0;
#ifdef _WIN32
    int length = static_cast<int>(sizeof(socket_error));
    const int result = getsockopt(socket, SOL_SOCKET, SO_ERROR,
                                  reinterpret_cast<char*>(&socket_error), &length);
#else
    socklen_t length = static_cast<socklen_t>(sizeof(socket_error));
    const int result = getsockopt(socket, SOL_SOCKET, SO_ERROR, &socket_error, &length);
#endif
    return finish(result == 0 && socket_error == 0);
}

std::string session_error(LIBSSH2_SESSION* session, std::string_view prefix) {
    char* message = nullptr;
    int length = 0;
    libssh2_session_last_error(session, &message, &length, 0);
    return std::string(prefix) +
           (message == nullptr ? "" : ": " + std::string(message, static_cast<std::size_t>(length)));
}

std::filesystem::path known_hosts_path() {
#ifdef _WIN32
    const char* home = std::getenv("USERPROFILE");
#else
    const char* home = std::getenv("HOME");
#endif
    if (home == nullptr || *home == '\0') {
        throw std::runtime_error("Home directory is unavailable for strict host-key verification.");
    }
    return std::filesystem::path(home) / ".ssh" / "known_hosts";
}

int known_key_mask(const int type) {
    switch (type) {
        case LIBSSH2_HOSTKEY_TYPE_RSA:
            return LIBSSH2_KNOWNHOST_KEY_SSHRSA;
        case LIBSSH2_HOSTKEY_TYPE_ECDSA_256:
            return LIBSSH2_KNOWNHOST_KEY_ECDSA_256;
        case LIBSSH2_HOSTKEY_TYPE_ECDSA_384:
            return LIBSSH2_KNOWNHOST_KEY_ECDSA_384;
        case LIBSSH2_HOSTKEY_TYPE_ECDSA_521:
            return LIBSSH2_KNOWNHOST_KEY_ECDSA_521;
        case LIBSSH2_HOSTKEY_TYPE_ED25519:
            return LIBSSH2_KNOWNHOST_KEY_ED25519;
        default:
            return LIBSSH2_KNOWNHOST_KEY_UNKNOWN;
    }
}

void append_bounded(std::string& target, const std::size_t other_size,
                    const char* bytes, const std::size_t count,
                    const std::function<void(std::string_view)>& progress) {
    if (target.size() + other_size + count > max_remote_output) {
        throw std::runtime_error("Remote command output exceeded 4 MiB.");
    }
    target.append(bytes, count);
    if (progress) {
        progress(std::string_view(bytes, count));
    }
}

struct LibsshGlobal final {
    LibsshGlobal() {
#ifdef _WIN32
        WSADATA data{};
        if (WSAStartup(MAKEWORD(2, 2), &data) != 0) {
            throw std::runtime_error("Unable to initialize WinSock.");
        }
#endif
        if (libssh2_init(0) != 0) {
            throw std::runtime_error("Unable to initialize libssh2.");
        }
    }
    ~LibsshGlobal() {
        libssh2_exit();
#ifdef _WIN32
        WSACleanup();
#endif
    }
};

LibsshGlobal& global_state() {
    static LibsshGlobal state;
    return state;
}

}  // namespace

struct SshClient::Implementation final {
    explicit Implementation(const Connection& connection) {
        try {
            (void)global_state();
            addrinfo hints{};
            hints.ai_family = AF_UNSPEC;
            hints.ai_socktype = SOCK_STREAM;
            addrinfo* addresses = nullptr;
            const auto service = std::to_string(connection.port);
            if (getaddrinfo(connection.host.c_str(), service.c_str(), &hints, &addresses) != 0) {
                throw std::runtime_error("Unable to resolve the SSH host.");
            }
            for (auto* address = addresses; address != nullptr; address = address->ai_next) {
                socket = ::socket(address->ai_family, address->ai_socktype, address->ai_protocol);
                if (socket == invalid_socket) {
                    continue;
                }
                if (connect_with_timeout(socket, *address)) {
                    break;
                }
                close_socket(socket);
                socket = invalid_socket;
            }
            freeaddrinfo(addresses);
            if (socket == invalid_socket) {
                throw std::runtime_error("Unable to connect to the SSH host.");
            }
            session = libssh2_session_init();
            if (session == nullptr) {
                throw std::runtime_error("Unable to allocate an SSH session.");
            }
            libssh2_session_set_blocking(session, 1);
            libssh2_session_set_timeout(session, 60'000);
            if (libssh2_session_handshake(session, socket) != 0) {
                throw std::runtime_error(session_error(session, "SSH handshake failed"));
            }
            verify_host(connection);
            int authentication = -1;
            if (connection.auth == AuthMethod::password) {
                authentication = libssh2_userauth_password_ex(
                    session, connection.username.c_str(),
                    static_cast<unsigned int>(connection.username.size()), connection.password.c_str(),
                    static_cast<unsigned int>(connection.password.size()), nullptr);
            } else {
                authentication = libssh2_userauth_publickey_fromfile_ex(
                    session, connection.username.c_str(),
                    static_cast<unsigned int>(connection.username.size()), nullptr,
                    connection.key_path.string().c_str(),
                    connection.key_passphrase.empty() ? nullptr : connection.key_passphrase.c_str());
            }
            if (authentication != 0) {
                throw std::runtime_error(session_error(session, "SSH authentication failed"));
            }
        } catch (...) {
            close();
            throw;
        }
    }

    ~Implementation() { close(); }

    void close() noexcept {
        if (session != nullptr) {
            libssh2_session_disconnect(session, "Raspberry Pi OMT deployer closed the session");
            libssh2_session_free(session);
            session = nullptr;
        }
        if (socket != invalid_socket) {
            close_socket(socket);
            socket = invalid_socket;
        }
    }

    void verify_host(const Connection& connection) {
        const auto path = known_hosts_path();
        if (!std::filesystem::is_regular_file(path)) {
            throw std::runtime_error(
                "Strict host-key verification requires ~/.ssh/known_hosts. Add the Pi key first.");
        }
        LIBSSH2_KNOWNHOSTS* hosts = libssh2_knownhost_init(session);
        if (hosts == nullptr) {
            throw std::runtime_error("Unable to initialize SSH known-host verification.");
        }
        const auto cleanup = [&hosts] { libssh2_knownhost_free(hosts); };
        if (libssh2_knownhost_readfile(hosts, path.string().c_str(), LIBSSH2_KNOWNHOST_FILE_OPENSSH) < 0) {
            cleanup();
            throw std::runtime_error("Unable to read ~/.ssh/known_hosts.");
        }
        std::size_t key_length = 0;
        int key_type = 0;
        const char* key = libssh2_session_hostkey(session, &key_length, &key_type);
        libssh2_knownhost* matched = nullptr;
        const int result = key == nullptr
                               ? LIBSSH2_KNOWNHOST_CHECK_FAILURE
                               : libssh2_knownhost_checkp(
                                     hosts, connection.host.c_str(), connection.port, key, key_length,
                                     LIBSSH2_KNOWNHOST_TYPE_PLAIN | LIBSSH2_KNOWNHOST_KEYENC_RAW |
                                         known_key_mask(key_type),
                                     &matched);
        cleanup();
        if (result != LIBSSH2_KNOWNHOST_CHECK_MATCH) {
            throw std::runtime_error(
                "The SSH host key is unknown or changed; strict verification refused the connection.");
        }
    }

    Socket socket{invalid_socket};
    LIBSSH2_SESSION* session{};
};

SshClient::SshClient(const Connection& connection)
    : implementation_(std::make_unique<Implementation>(connection)) {}
SshClient::~SshClient() = default;
SshClient::SshClient(SshClient&&) noexcept = default;
SshClient& SshClient::operator=(SshClient&&) noexcept = default;

RemoteResult SshClient::run(const std::string_view command, const std::string_view input,
                            const std::function<void(std::string_view)>& progress) {
    auto* session = implementation_->session;
    LIBSSH2_CHANNEL* channel = libssh2_channel_open_session(session);
    if (channel == nullptr) {
        throw std::runtime_error(session_error(session, "Unable to open remote command channel"));
    }
    const auto close_channel = [&channel] {
        libssh2_channel_close(channel);
        libssh2_channel_free(channel);
    };
    const std::string remote_command(command);
    if (libssh2_channel_process_startup(channel, "exec", 4, remote_command.c_str(),
                                        static_cast<unsigned int>(remote_command.size())) != 0) {
        close_channel();
        throw std::runtime_error(session_error(session, "Unable to start remote command"));
    }
    std::size_t written = 0;
    while (written < input.size()) {
        const auto count = libssh2_channel_write(channel, input.data() + written, input.size() - written);
        if (count <= 0) {
            close_channel();
            throw std::runtime_error(session_error(session, "Unable to write remote command input"));
        }
        written += static_cast<std::size_t>(count);
    }
    libssh2_channel_send_eof(channel);
    libssh2_session_set_blocking(session, 0);
    const auto blocking_cleanup = [&] { libssh2_session_set_blocking(session, 1); };
    RemoteResult result;
    std::array<char, 16 * 1024> buffer{};
    auto inactivity_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(60);
    for (;;) {
        bool received = false;
        const auto output_count = libssh2_channel_read(channel, buffer.data(), buffer.size());
        if (output_count > 0) {
            try {
                append_bounded(result.output, result.error.size(), buffer.data(),
                               static_cast<std::size_t>(output_count), progress);
            } catch (...) {
                blocking_cleanup();
                close_channel();
                throw;
            }
            received = true;
        } else if (output_count < 0 && output_count != LIBSSH2_ERROR_EAGAIN) {
            blocking_cleanup();
            close_channel();
            throw std::runtime_error(session_error(session, "Unable to read remote output"));
        }
        const auto error_count = libssh2_channel_read_stderr(channel, buffer.data(), buffer.size());
        if (error_count > 0) {
            try {
                append_bounded(result.error, result.output.size(), buffer.data(),
                               static_cast<std::size_t>(error_count), progress);
            } catch (...) {
                blocking_cleanup();
                close_channel();
                throw;
            }
            received = true;
        } else if (error_count < 0 && error_count != LIBSSH2_ERROR_EAGAIN) {
            blocking_cleanup();
            close_channel();
            throw std::runtime_error(session_error(session, "Unable to read remote error output"));
        }
        if (received) {
            inactivity_deadline = std::chrono::steady_clock::now() + std::chrono::seconds(60);
        }
        if (libssh2_channel_eof(channel) != 0) {
            break;
        }
        if (!received) {
            if (std::chrono::steady_clock::now() >= inactivity_deadline) {
                blocking_cleanup();
                close_channel();
                throw std::runtime_error("Remote command produced no output for 60 seconds.");
            }
            fd_set read_set;
            fd_set write_set;
            FD_ZERO(&read_set);
            FD_ZERO(&write_set);
            const int directions = libssh2_session_block_directions(session);
            if ((directions & LIBSSH2_SESSION_BLOCK_INBOUND) != 0) FD_SET(implementation_->socket, &read_set);
            if ((directions & LIBSSH2_SESSION_BLOCK_OUTBOUND) != 0) FD_SET(implementation_->socket, &write_set);
            if (directions == 0) FD_SET(implementation_->socket, &read_set);
            timeval timeout{1, 0};
#ifdef _WIN32
            (void)::select(0, &read_set, &write_set, nullptr, &timeout);
#else
            (void)::select(implementation_->socket + 1, &read_set, &write_set, nullptr, &timeout);
#endif
        }
    }
    result.exit_code = libssh2_channel_get_exit_status(channel);
    blocking_cleanup();
    close_channel();
    return result;
}

void SshClient::upload(const std::filesystem::path& local_path, const std::string_view remote_path,
                       const std::function<void(std::uint64_t, std::uint64_t)>& progress) {
    auto* session = implementation_->session;
    LIBSSH2_SFTP* sftp = libssh2_sftp_init(session);
    if (sftp == nullptr) {
        throw std::runtime_error(session_error(session, "Unable to initialize SFTP"));
    }
    std::ifstream input(local_path, std::ios::binary);
    if (!input) {
        libssh2_sftp_shutdown(sftp);
        throw std::runtime_error("Unable to open local deployment artifact.");
    }
    const auto total = std::filesystem::file_size(local_path);
    const std::string remote(remote_path);
    LIBSSH2_SFTP_HANDLE* output = libssh2_sftp_open_ex(
        sftp, remote.c_str(), static_cast<unsigned int>(remote.size()),
        LIBSSH2_FXF_WRITE | LIBSSH2_FXF_CREAT | LIBSSH2_FXF_TRUNC,
        LIBSSH2_SFTP_S_IRUSR | LIBSSH2_SFTP_S_IWUSR, LIBSSH2_SFTP_OPENFILE);
    if (output == nullptr) {
        libssh2_sftp_shutdown(sftp);
        throw std::runtime_error(session_error(session, "Unable to create remote deployment artifact"));
    }
    std::array<char, 64 * 1024> buffer{};
    std::uint64_t uploaded = 0;
    try {
        while (input) {
            input.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
            std::size_t offset = 0;
            const auto available = static_cast<std::size_t>(input.gcount());
            while (offset < available) {
                const auto count = libssh2_sftp_write(output, buffer.data() + offset, available - offset);
                if (count <= 0) {
                    throw std::runtime_error(session_error(session, "SFTP upload failed"));
                }
                offset += static_cast<std::size_t>(count);
                uploaded += static_cast<std::uint64_t>(count);
                if (progress) progress(uploaded, total);
            }
        }
    } catch (...) {
        libssh2_sftp_close(output);
        libssh2_sftp_shutdown(sftp);
        throw;
    }
    libssh2_sftp_close(output);
    libssh2_sftp_shutdown(sftp);
}

}  // namespace omt::deployer
