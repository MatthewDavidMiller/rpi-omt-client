#include "core.hpp"
#include "process.hpp"
#include "sha256.hpp"

#include <cassert>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>

int main() {
    using namespace omt::deployer;
    assert(valid_host("pi.local"));
    assert(valid_host("192.168.1.20"));
    assert(!valid_host("-pi.local"));
    assert(!valid_host("pi..local"));
    assert(valid_username("pi_admin-1"));
    assert(!valid_username("pi admin"));
    Connection invalid_port;
    invalid_port.host = "pi.local";
    invalid_port.username = "admin";
    invalid_port.password = "password";
    invalid_port.port = 0;
    assert(!validate(invalid_port).empty());
    Connection oversized_secret;
    oversized_secret.host = "pi.local";
    oversized_secret.username = "admin";
    oversized_secret.password.assign(4097, 'x');
    assert(!validate(oversized_secret).empty());
    assert(valid_remote_directory("/opt/omt-client"));
    assert(!valid_remote_directory("/opt/../root"));
    assert(shell_quote("a'b") == "'a'\\''b'");

    const auto token = random_token(16);
    assert(token.size() == 32);
    assert(token.find_first_not_of("0123456789abcdef") == std::string::npos);
    const auto root = std::filesystem::temp_directory_path() / ("omt-deployer-core-test-" + token);
    std::filesystem::create_directories(root / "deploy");
    {
        std::ofstream manifest(root / "deploy" / "manifest-v3.txt");
        manifest << "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n";
    }
    const auto names = load_manifest(root / "deploy" / "manifest-v3.txt");
    assert(names.size() == 2);
    {
        std::ofstream sample(root / "sample.txt", std::ios::binary);
        sample << "abc";
    }
    assert(sha256_file(root / "sample.txt") ==
           "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    std::filesystem::remove_all(root);

    WifiSettings wifi{"office", "12345678", true};
    assert(wifi_error(wifi).empty());
    wifi.password = "short";
    assert(!wifi_error(wifi).empty());
    wifi.ssid = std::string("bad\xC0\xAF", 5);
    assert(!wifi_error(wifi).empty());

#ifdef _WIN32
    const std::vector<std::string> success_command{"cmd.exe", "/d", "/s", "/c", "echo ok"};
    const std::vector<std::string> cancel_command{
        "cmd.exe", "/d", "/s", "/c", "ping -n 30 127.0.0.1 >nul"};
#else
    const std::vector<std::string> success_command{"/bin/sh", "-c", "printf ok"};
    const std::vector<std::string> cancel_command{"/bin/sh", "-c", "while :; do sleep 1; done"};
#endif
    const auto process = run_process(success_command, std::filesystem::temp_directory_path());
    assert(process.exit_code == 0 && process.output.find("ok") != std::string::npos);
    bool cancelled = false;
    try {
        (void)run_process(cancel_command, std::filesystem::temp_directory_path(), {}, [] {
            return true;
        });
    } catch (const std::runtime_error&) {
        cancelled = true;
    }
    assert(cancelled);
    std::cout << "native deployer core contracts passed\n";
}
