#include "process.hpp"

#include <algorithm>
#include <array>
#include <limits>
#include <stdexcept>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <cerrno>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

namespace omt::deployer {
namespace {
constexpr std::size_t max_output = 4U * 1024U * 1024U;

void append_output(std::string& output, const char* data, const std::size_t size,
                   const Progress& progress) {
    if (output.size() + size > max_output) {
        throw std::runtime_error("Child process output exceeded 4 MiB.");
    }
    output.append(data, size);
    if (progress) progress(std::string_view(data, size));
}

#ifdef _WIN32
std::wstring utf16(const std::string& value) {
    if (value.empty()) return {};
    if (value.size() > static_cast<std::size_t>(std::numeric_limits<int>::max())) {
        throw std::runtime_error("Process argument is too long.");
    }
    const int length = static_cast<int>(value.size());
    const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), length,
                                         nullptr, 0);
    if (size <= 0) throw std::runtime_error("Invalid UTF-8 process argument.");
    std::wstring result(static_cast<std::size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), length, result.data(), size);
    return result;
}

std::wstring quote_windows(const std::string& value) {
    const auto wide = utf16(value);
    if (wide.empty()) return L"\"\"";
    if (wide.find_first_of(L" \t\"") == std::wstring::npos) return wide;
    std::wstring result{L'\"'};
    std::size_t slashes = 0;
    for (const wchar_t character : wide) {
        if (character == L'\\') {
            ++slashes;
        } else if (character == L'\"') {
            result.append(slashes * 2 + 1, L'\\');
            result += character;
            slashes = 0;
        } else {
            result.append(slashes, L'\\');
            result += character;
            slashes = 0;
        }
    }
    result.append(slashes * 2, L'\\');
    result += L'\"';
    return result;
}
#endif
}  // namespace

ProcessResult run_process(const std::vector<std::string>& arguments,
                          const std::filesystem::path& working_directory,
                          const Progress& progress, const StopRequested& stop_requested) {
    if (arguments.empty()) throw std::invalid_argument("process command is empty");
#ifdef _WIN32
    SECURITY_ATTRIBUTES attributes{sizeof(attributes), nullptr, TRUE};
    HANDLE read_handle = nullptr;
    HANDLE write_handle = nullptr;
    if (CreatePipe(&read_handle, &write_handle, &attributes, 0) == 0) {
        throw std::runtime_error("Unable to create process output pipe.");
    }
    if (SetHandleInformation(read_handle, HANDLE_FLAG_INHERIT, 0) == 0) {
        CloseHandle(read_handle);
        CloseHandle(write_handle);
        throw std::runtime_error("Unable to protect the process output pipe.");
    }
    HANDLE job = CreateJobObjectW(nullptr, nullptr);
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if (job == nullptr ||
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits, sizeof(limits)) == 0) {
        if (job != nullptr) CloseHandle(job);
        CloseHandle(read_handle);
        CloseHandle(write_handle);
        throw std::runtime_error("Unable to create a bounded child-process job.");
    }
    HANDLE input_handle = CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
                                      &attributes, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (input_handle == INVALID_HANDLE_VALUE) {
        CloseHandle(job);
        CloseHandle(read_handle);
        CloseHandle(write_handle);
        throw std::runtime_error("Unable to create a safe child-process input handle.");
    }
    std::wstring command;
    for (const auto& argument : arguments) {
        if (!command.empty()) command += L' ';
        command += quote_windows(argument);
    }
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdOutput = write_handle;
    startup.hStdError = write_handle;
    startup.hStdInput = input_handle;
    PROCESS_INFORMATION process{};
    auto directory = working_directory.wstring();
    const BOOL created = CreateProcessW(
        nullptr, command.data(), nullptr, nullptr, TRUE, CREATE_NO_WINDOW | CREATE_SUSPENDED,
        nullptr, directory.c_str(), &startup, &process);
    CloseHandle(input_handle);
    CloseHandle(write_handle);
    if (created == 0 || AssignProcessToJobObject(job, process.hProcess) == 0 ||
        ResumeThread(process.hThread) == static_cast<DWORD>(-1)) {
        if (created != 0) {
            TerminateProcess(process.hProcess, 126);
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
        }
        CloseHandle(read_handle);
        CloseHandle(job);
        throw std::runtime_error("Unable to start the child process safely.");
    }
    ProcessResult result;
    std::array<char, 16 * 1024> buffer{};
    bool cancelled = false;
    try {
        for (;;) {
            if (stop_requested && stop_requested()) {
                cancelled = true;
                TerminateJobObject(job, ERROR_CANCELLED);
            }
            DWORD available = 0;
            if (PeekNamedPipe(read_handle, nullptr, 0, nullptr, &available, nullptr) != 0 &&
                available > 0) {
                DWORD count = 0;
                const DWORD wanted = std::min<DWORD>(available, static_cast<DWORD>(buffer.size()));
                if (ReadFile(read_handle, buffer.data(), wanted, &count, nullptr) != 0 && count > 0) {
                    append_output(result.output, buffer.data(), static_cast<std::size_t>(count), progress);
                }
            }
            if (WaitForSingleObject(process.hProcess, 50) == WAIT_OBJECT_0) {
                DWORD remaining = 0;
                if (PeekNamedPipe(read_handle, nullptr, 0, nullptr, &remaining, nullptr) == 0 ||
                    remaining == 0) break;
            }
        }
    } catch (...) {
        TerminateJobObject(job, 126);
        WaitForSingleObject(process.hProcess, 5'000);
        CloseHandle(read_handle);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        CloseHandle(job);
        throw;
    }
    DWORD exit_code = 1;
    GetExitCodeProcess(process.hProcess, &exit_code);
    result.exit_code = static_cast<int>(exit_code);
    CloseHandle(read_handle);
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    CloseHandle(job);
    if (cancelled) throw std::runtime_error("Operation cancelled.");
    return result;
#else
    int descriptors[2]{};
    if (pipe(descriptors) != 0) throw std::runtime_error("Unable to create process output pipe.");
    (void)fcntl(descriptors[0], F_SETFD, FD_CLOEXEC);
    (void)fcntl(descriptors[1], F_SETFD, FD_CLOEXEC);
    std::vector<char*> argv;
    argv.reserve(arguments.size() + 1);
    for (const auto& argument : arguments) argv.push_back(const_cast<char*>(argument.c_str()));
    argv.push_back(nullptr);
    const pid_t child = fork();
    if (child < 0) {
        close(descriptors[0]);
        close(descriptors[1]);
        throw std::runtime_error("Unable to fork child process.");
    }
    if (child == 0) {
        (void)setpgid(0, 0);
        close(descriptors[0]);
        if (chdir(working_directory.c_str()) != 0 || dup2(descriptors[1], STDOUT_FILENO) < 0 ||
            dup2(descriptors[1], STDERR_FILENO) < 0) {
            _exit(126);
        }
        close(descriptors[1]);
        execvp(argv[0], argv.data());
        _exit(errno == ENOENT ? 127 : 126);
    }
    close(descriptors[1]);
    (void)setpgid(child, child);
    const int flags = fcntl(descriptors[0], F_GETFL, 0);
    if (flags < 0 || fcntl(descriptors[0], F_SETFL, flags | O_NONBLOCK) < 0) {
        (void)kill(-child, SIGKILL);
        (void)kill(child, SIGKILL);
        close(descriptors[0]);
        (void)waitpid(child, nullptr, 0);
        throw std::runtime_error("Unable to configure the process output pipe.");
    }
    bool reaped = false;
    int status = 0;
    const auto terminate = [&] {
        if (!reaped) {
            (void)kill(-child, SIGTERM);
            (void)kill(child, SIGTERM);
            for (int attempt = 0; attempt < 10; ++attempt) {
                if (waitpid(child, &status, WNOHANG) == child) {
                    reaped = true;
                    break;
                }
                (void)poll(nullptr, 0, 50);
            }
            if (!reaped) {
                (void)kill(-child, SIGKILL);
                (void)kill(child, SIGKILL);
                while (waitpid(child, &status, 0) < 0 && errno == EINTR) {
                }
                reaped = true;
            }
        }
    };
    ProcessResult result;
    std::array<char, 16 * 1024> buffer{};
    bool pipe_closed = false;
    try {
        while (!reaped || !pipe_closed) {
            if (stop_requested && stop_requested()) {
                throw std::runtime_error("Operation cancelled.");
            }
            pollfd descriptor{descriptors[0], POLLIN, 0};
            (void)poll(&descriptor, 1, 100);
            for (;;) {
                const auto count = read(descriptors[0], buffer.data(), buffer.size());
                if (count > 0) {
                    append_output(result.output, buffer.data(), static_cast<std::size_t>(count), progress);
                } else if (count == 0) {
                    pipe_closed = true;
                    break;
                } else if (errno == EINTR) {
                    continue;
                } else if (errno == EAGAIN || errno == EWOULDBLOCK) {
                    break;
                } else {
                    throw std::runtime_error("Unable to read child process output.");
                }
            }
            if (!reaped && waitpid(child, &status, WNOHANG) == child) reaped = true;
        }
    } catch (...) {
        terminate();
        close(descriptors[0]);
        throw;
    }
    close(descriptors[0]);
    result.exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : 128;
    return result;
#endif
}

}  // namespace omt::deployer
