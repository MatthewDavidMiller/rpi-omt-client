#include "core.hpp"
#include "legal_texts.hpp"
#include "process.hpp"
#include "sha256.hpp"

#include <cassert>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace {

/// Write `body` to `path` and return whether `load_manifest` accepted it.
///
/// Manifest v3 names every path the deployer uploads and `transaction.sh` then
/// promotes into the install directory as root. A name that escaped this
/// validation would be written outside the staging tree on the Pi, so each
/// rejection below is a boundary rather than a formatting preference.
bool manifest_is_accepted(const std::filesystem::path& path, std::string_view body) {
    {
        std::ofstream manifest(path, std::ios::binary | std::ios::trunc);
        manifest << body;
    }
    try {
        (void)omt::deployer::load_manifest(path);
        return true;
    } catch (const std::runtime_error&) {
        return false;
    }
}

void manifest_contract(const std::filesystem::path& path) {
    using namespace std::string_literals;
    static constexpr std::string_view required =
        "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n";

    assert(manifest_is_accepted(path, required));
    assert(manifest_is_accepted(
        path, "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\ndeploy/host/install.sh\n"));

    // The version line is the schema gate; a v2 capsule must not be read as v3.
    assert(!manifest_is_accepted(path, "deploy/transaction.sh\ndeploy/manifest-v3.txt\n"));
    assert(!manifest_is_accepted(path, "version=2\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n"));
    assert(!manifest_is_accepted(path, "version=3 \ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n"));
    assert(!manifest_is_accepted(path, ""));

    // Both members the transaction itself needs must be present.
    assert(!manifest_is_accepted(path, "version=3\n"));
    assert(!manifest_is_accepted(path, "version=3\ndeploy/transaction.sh\n"));
    assert(!manifest_is_accepted(path, "version=3\ndeploy/manifest-v3.txt\n"));

    // Nothing may name a path outside the staging tree, and no name may repeat:
    // a duplicate would be uploaded and hashed twice with no way to tell which
    // copy the promotion used.
    for (const auto& unsafe : {"../etc/passwd"s, "deploy/../../etc/passwd"s, "/etc/passwd"s,
                               "deploy//transaction.sh"s, "deploy/"s, "/"s, "."s, ".."s,
                               "deploy/./transaction.sh"s, "deploy\\transaction.sh"s,
                               "deploy/trans action.sh"s, "deploy/transaction.sh\r"s,
                               "deploy/caf\xc3\xa9.sh"s, std::string(241, 'x')}) {
        assert(!manifest_is_accepted(path, std::string(required) + unsafe + "\n"));
    }
    assert(!manifest_is_accepted(path, std::string(required) + "deploy/transaction.sh\n"));

    // The capsule is bounded, so a manifest cannot ask for an unbounded upload.
    std::string oversized(required);
    for (int index = 0; index < 200; ++index) {
        oversized += "deploy/file" + std::to_string(index) + ".txt\n";
    }
    assert(!manifest_is_accepted(path, oversized));

    // A manifest that is not a plain regular file is unusable, not empty.
    std::filesystem::remove(path);
    bool rejected = false;
    try {
        (void)omt::deployer::load_manifest(path);
    } catch (const std::runtime_error&) {
        rejected = true;
    }
    assert(rejected);
}

/// The deployer is started from an installed package or a desktop shortcut as
/// often as from a checkout, so it locates the tree it uploads itself.
/// `root` already holds `deploy/manifest-v3.txt` from the manifest suite.
void project_root_contract(const std::filesystem::path& root) {
    using namespace omt::deployer;
    const auto binaries = root / "package" / "bin";
    std::filesystem::create_directories(binaries);

    assert(discover_project_root(binaries, root / "deploy") == root);
    assert(discover_project_root(binaries, {}) == root);
    assert(discover_project_root({}, root) == root);
    assert(discover_project_root({}, {}).empty());

    // SDL reports the executable's directory with a trailing separator, which
    // leaves an empty filename component: `parent_path()` then returns the
    // directory itself and the ascent stalls on its first step.
    assert(discover_project_root(binaries.string() + "/", {}) == root);
    assert(discover_project_root({}, root.string() + "/") == root);

    // The ascent is bounded, so a marker far above an unrelated directory is
    // not adopted as the project root.
    auto deep = root;
    for (int level = 0; level < 10; ++level) {
        deep /= "level";
    }
    assert(discover_project_root({}, deep) == deep);
    std::filesystem::remove_all(root / "package");
}

/// The legal texts are compiled in, so About cannot be left with nothing to
/// show by a package that travelled without its files. This asserts the
/// generated translation unit actually carries the repository's texts.
void legal_contract() {
    const auto documents = omt::deployer::legal_documents();
    assert(documents.size() == 2);
    assert(documents[0].name == "LICENSE");
    assert(documents[1].name == "THIRD_PARTY_NOTICES.txt");
    assert(documents[0].text.find("MIT License") != std::string_view::npos);
    assert(documents[0].text.find("Matthew David Miller") != std::string_view::npos);
    assert(documents[1].text.find("Dear ImGui") != std::string_view::npos);
    for (const auto& document : documents) {
        // Text, not a truncated or NUL-padded blob: About renders it verbatim.
        assert(document.text.size() > 512);
        assert(document.text.find('\0') == std::string_view::npos);
        assert(document.text.back() == '\n');
    }
}

}  // namespace

int main() {
    using namespace omt::deployer;
    assert(valid_host("pi.local"));
    assert(valid_host("192.168.1.20"));
    assert(!valid_host("-pi.local"));
    assert(!valid_host("pi..local"));
    assert(!valid_host(""));
    assert(!valid_host("pi.local."));
    assert(!valid_host("pi local"));
    assert(!valid_host(std::string(254, 'a')));
    assert(valid_username("pi_admin-1"));
    assert(!valid_username("pi admin"));
    assert(!valid_username(""));
    assert(!valid_username("pi/admin"));
    assert(!valid_username(std::string(65, 'a')));
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
    assert(!valid_remote_directory("/"));
    assert(!valid_remote_directory("opt/omt-client"));
    assert(!valid_remote_directory("/opt/omt-client/"));
    assert(!valid_remote_directory("/opt//omt-client"));
    assert(!valid_remote_directory("/opt/omt client"));

    // Single quoting is the only escape a POSIX shell honours inside '...', so
    // every argument the deployer interpolates into a remote command relies on
    // this exact form.
    assert(shell_quote("a'b") == "'a'\\''b'");
    assert(shell_quote("") == "''");
    assert(shell_quote("plain") == "'plain'");
    assert(shell_quote("$(id)") == "'$(id)'");
    assert(shell_quote("a b;rm -rf /") == "'a b;rm -rf /'");

    const auto token = random_token(16);
    assert(token.size() == 32);
    assert(token.find_first_not_of("0123456789abcdef") == std::string::npos);
    assert(random_token(16) != token);
    const auto root = std::filesystem::temp_directory_path() / ("omt-deployer-core-test-" + token);
    std::filesystem::create_directories(root / "deploy");
    manifest_contract(root / "deploy" / "manifest-v3.txt");
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
    project_root_contract(root);
    legal_contract();
    std::filesystem::remove_all(root);

    // WPA2 accepts either an 8-63 character passphrase or a 64-digit hex PSK;
    // anything else is rejected here rather than by wpa_supplicant on a Pi that
    // has just lost its network.
    WifiSettings wifi{"office", "12345678", true};
    assert(wifi_error(wifi).empty());
    wifi.password = std::string(63, 'p');
    assert(wifi_error(wifi).empty());
    wifi.password = std::string(64, 'a');
    assert(wifi_error(wifi).empty());
    wifi.password = "short";
    assert(!wifi_error(wifi).empty());
    wifi.password = std::string(64, 'z');
    assert(!wifi_error(wifi).empty());
    wifi.password = std::string(65, 'a');
    assert(!wifi_error(wifi).empty());
    wifi.password = "with\nnewline";
    assert(!wifi_error(wifi).empty());
    wifi.password = "12345678";
    wifi.ssid = "";
    assert(!wifi_error(wifi).empty());
    wifi.ssid = std::string(33, 's');
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
