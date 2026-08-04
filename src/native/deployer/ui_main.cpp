#include "deployment.hpp"

#include <SDL3/SDL.h>
#include <backends/imgui_impl_sdl3.h>
#include <backends/imgui_impl_sdlrenderer3.h>
#include <imgui.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <charconv>
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <future>
#include <mutex>
#include <sstream>
#include <string>
#include <system_error>

namespace {
template <std::size_t Size>
std::string value(const std::array<char, Size>& buffer) {
    return std::string(buffer.data());
}

template <std::size_t Size>
void set_value(std::array<char, Size>& buffer, const std::string& text) {
    const auto count = std::min(text.size(), buffer.size() - 1);
    std::copy_n(text.data(), count, buffer.data());
    buffer[count] = '\0';
}

template <std::size_t Size>
void clear_value(std::array<char, Size>& buffer) noexcept {
    volatile char* memory = buffer.data();
    for (std::size_t index = 0; index < buffer.size(); ++index) {
        memory[index] = '\0';
    }
}

std::string bounded_file(const std::filesystem::path& path) {
    std::ifstream input(path, std::ios::binary);
    if (!input) {
        return "Legal file unavailable: " + path.string();
    }
    std::ostringstream output;
    std::array<char, 16 * 1024> buffer{};
    std::size_t total = 0;
    while (input && total < 2U * 1024U * 1024U) {
        input.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        const auto count = static_cast<std::size_t>(input.gcount());
        output.write(buffer.data(), static_cast<std::streamsize>(count));
        total += count;
    }
    return output.str();
}

struct Application final {
    Application() {
        set_value(project_root, std::filesystem::current_path().string());
        set_value(remote_directory, "/opt/omt-client");
        set_value(port, "22");
        set_value(username, "admin");
    }

    ~Application() {
        cancel_requested.store(true);
        if (task.valid()) {
            task.wait();
        }
        clear_value(password);
        clear_value(key_passphrase);
        clear_value(sudo_password);
        clear_value(wifi_password);
    }

    omt::deployer::Connection connection() const {
        omt::deployer::Connection result;
        result.host = value(host);
        result.username = value(username);
        const std::string port_text = value(port);
        unsigned int parsed_port = 0;
        const auto parsed = std::from_chars(
            port_text.data(), port_text.data() + port_text.size(), parsed_port, 10);
        result.port = parsed.ec == std::errc{} && parsed.ptr == port_text.data() + port_text.size() &&
                              parsed_port >= 1U && parsed_port <= 65'535U
                          ? static_cast<std::uint16_t>(parsed_port)
                          : 0;
        result.auth = use_key ? omt::deployer::AuthMethod::key
                              : omt::deployer::AuthMethod::password;
        result.password = value(password);
        result.key_path = value(key_path);
        result.key_passphrase = value(key_passphrase);
        result.sudo_password = value(sudo_password);
        return result;
    }

    omt::deployer::Options options() const {
        omt::deployer::Options result;
        result.project_root = value(project_root);
        result.remote_directory = value(remote_directory);
        result.build_image = build_image;
        return result;
    }

    omt::deployer::WifiSettings wifi() const {
        return {value(wifi_ssid), value(wifi_password), wifi_connect};
    }

    bool idle() {
        if (!task.valid()) {
            return true;
        }
        if (task.wait_for(std::chrono::seconds(0)) != std::future_status::ready) {
            return false;
        }
        try {
            task.get();
            append("Operation completed.\n");
        } catch (const std::exception& exception) {
            append(std::string("ERROR: ") + exception.what() + "\n");
        }
        cancel_requested.store(false);
        return true;
    }

    template <typename Function>
    void start(Function&& function) {
        if (!idle()) {
            return;
        }
        append("\n--- Starting operation ---\n");
        cancel_requested.store(false);
        task = std::async(std::launch::async, std::forward<Function>(function));
    }

    void append(std::string_view message) {
        std::scoped_lock lock(log_mutex);
        if (log.size() + message.size() > 4U * 1024U * 1024U) {
            log.erase(0, log.size() + message.size() - 4U * 1024U * 1024U);
        }
        log.append(message);
    }

    void draw() {
        const bool available = idle();
        ImGui::SetNextWindowPos({0, 0});
        ImGui::SetNextWindowSize(ImGui::GetIO().DisplaySize);
        ImGui::Begin("Raspberry Pi OMT Deployer", nullptr,
                     ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoResize |
                         ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoTitleBar);
        ImGui::TextUnformatted("Raspberry Pi OMT Deployer");
        ImGui::SameLine();
        ImGui::TextDisabled("native %s", OMT_CLIENT_VERSION);
        ImGui::Separator();
        if (ImGui::BeginTabBar("sections")) {
            draw_connection(available);
            draw_deployment(available);
            draw_wifi(available);
            draw_about();
            ImGui::EndTabBar();
        }
        if (!available && ImGui::Button("Cancel operation")) {
            cancel_requested.store(true);
            append("Cancellation requested; waiting for the current safe boundary.\n");
        }
        ImGui::SeparatorText(available ? "Activity" : "Activity - operation in progress");
        std::string snapshot;
        {
            std::scoped_lock lock(log_mutex);
            snapshot = log;
        }
        ImGui::InputTextMultiline("##log", snapshot.data(), snapshot.size() + 1,
                                  ImVec2(-1, -1), ImGuiInputTextFlags_ReadOnly);
        ImGui::End();
    }

    void draw_connection(bool available) {
        if (!ImGui::BeginTabItem("Connection")) {
            return;
        }
        ImGui::InputText("Pi host", host.data(), host.size());
        ImGui::InputText("SSH username", username.data(), username.size());
        ImGui::InputText("SSH port", port.data(), port.size(), ImGuiInputTextFlags_CharsDecimal);
        ImGui::Checkbox("Use private key", &use_key);
        if (use_key) {
            ImGui::InputText("Private key", key_path.data(), key_path.size());
            ImGui::InputText("Key passphrase", key_passphrase.data(), key_passphrase.size(),
                             ImGuiInputTextFlags_Password);
        } else {
            ImGui::InputText("SSH password", password.data(), password.size(),
                             ImGuiInputTextFlags_Password);
        }
        ImGui::InputText("Sudo password (optional)", sudo_password.data(), sudo_password.size(),
                         ImGuiInputTextFlags_Password);
        ImGui::BeginDisabled(!available);
        if (ImGui::Button("Test connection")) {
            const auto snapshot = connection();
            start([this, snapshot] {
                omt::deployer::DeploymentService service(OMT_CLIENT_VERSION,
                    [this](auto line) { append(line); },
                    [this] { return cancel_requested.load(); });
                service.test_connection(snapshot);
            });
        }
        ImGui::EndDisabled();
        ImGui::TextWrapped("Host keys must already exist in ~/.ssh/known_hosts. Changed or unknown keys are refused.");
        ImGui::EndTabItem();
    }

    void draw_deployment(bool available) {
        if (!ImGui::BeginTabItem("Deployment")) {
            return;
        }
        ImGui::InputText("Project root", project_root.data(), project_root.size());
        ImGui::InputText("Remote directory", remote_directory.data(), remote_directory.size());
        ImGui::Checkbox("Build ARM64 image", &build_image);
        const auto snapshot_connection = connection();
        const auto snapshot_options = options();
        ImGui::BeginDisabled(!available);
        if (ImGui::Button("Install prerequisites")) {
            const auto root = snapshot_options.project_root;
            start([this, root] {
                omt::deployer::DeploymentService service(OMT_CLIENT_VERSION,
                    [this](auto line) { append(line); },
                    [this] { return cancel_requested.load(); });
                service.install_prerequisites(root);
            });
        }
        ImGui::SameLine();
        if (ImGui::Button("Build and deploy")) {
            start([this, snapshot_connection, snapshot_options] {
                omt::deployer::DeploymentService service(OMT_CLIENT_VERSION,
                    [this](auto line) { append(line); },
                    [this] { return cancel_requested.load(); });
                service.deploy(snapshot_connection, snapshot_options);
            });
        }
        if (ImGui::Button("Status")) {
            start([this, snapshot_connection, snapshot_options] {
                omt::deployer::DeploymentService service(
                    OMT_CLIENT_VERSION, {}, [this] { return cancel_requested.load(); });
                append(service.status(snapshot_connection, snapshot_options.remote_directory));
            });
        }
        ImGui::SameLine();
        if (ImGui::Button("Recent logs")) {
            start([this, snapshot_connection, snapshot_options] {
                omt::deployer::DeploymentService service(
                    OMT_CLIENT_VERSION, {}, [this] { return cancel_requested.load(); });
                append(service.logs(snapshot_connection, snapshot_options.remote_directory));
            });
        }
        ImGui::SameLine();
        if (ImGui::Button("Restart service")) {
            start([this, snapshot_connection, snapshot_options] {
                omt::deployer::DeploymentService service(
                    OMT_CLIENT_VERSION, {}, [this] { return cancel_requested.load(); });
                append(service.restart(snapshot_connection, snapshot_options.remote_directory));
            });
        }
        ImGui::EndDisabled();
        ImGui::EndTabItem();
    }

    void draw_wifi(bool available) {
        if (!ImGui::BeginTabItem("Wi-Fi")) {
            return;
        }
        ImGui::InputText("SSID", wifi_ssid.data(), wifi_ssid.size());
        ImGui::InputText("Wi-Fi password", wifi_password.data(), wifi_password.size(),
                         ImGuiInputTextFlags_Password);
        ImGui::Checkbox("Connect immediately", &wifi_connect);
        ImGui::BeginDisabled(!available);
        if (ImGui::Button("Apply Wi-Fi settings")) {
            const auto snapshot_connection = connection();
            const auto snapshot_wifi = wifi();
            start([this, snapshot_connection, snapshot_wifi] {
                omt::deployer::DeploymentService service(OMT_CLIENT_VERSION,
                    [this](auto line) { append(line); },
                    [this] { return cancel_requested.load(); });
                service.apply_wifi(snapshot_connection, snapshot_wifi);
            });
        }
        ImGui::EndDisabled();
        ImGui::TextWrapped("Connecting can interrupt SSH if the Pi changes networks.");
        ImGui::EndTabItem();
    }

    void draw_about() {
        if (!ImGui::BeginTabItem("About")) {
            return;
        }
        ImGui::Text("Raspberry Pi OMT Client %s", OMT_CLIENT_VERSION);
        ImGui::TextUnformatted("Copyright (c) 2026 Matthew David Miller");
        ImGui::TextWrapped("Native C17/C++20 deployer using SDL3, Dear ImGui, and libssh2. Project code is MIT licensed.");
        if (legal_text.empty()) {
            const auto root = std::filesystem::path(value(project_root));
            legal_text = bounded_file(root / "LICENSE") + "\n\n" +
                         bounded_file(root / "THIRD_PARTY_NOTICES.txt");
        }
        ImGui::InputTextMultiline("##legal", legal_text.data(), legal_text.size() + 1,
                                  ImVec2(-1, -1), ImGuiInputTextFlags_ReadOnly);
        ImGui::EndTabItem();
    }

    std::array<char, 256> host{};
    std::array<char, 65> username{};
    std::array<char, 6> port{};
    std::array<char, 512> password{};
    std::array<char, 1024> key_path{};
    std::array<char, 512> key_passphrase{};
    std::array<char, 512> sudo_password{};
    std::array<char, 2048> project_root{};
    std::array<char, 256> remote_directory{};
    std::array<char, 64> wifi_ssid{};
    std::array<char, 128> wifi_password{};
    bool use_key{};
    bool build_image{true};
    bool wifi_connect{true};
    std::future<void> task;
    std::atomic_bool cancel_requested{};
    std::mutex log_mutex;
    std::string log{"Ready.\n"};
    std::string legal_text;
};
}  // namespace

int main(int, char**) {
    if (!SDL_Init(SDL_INIT_VIDEO)) {
        return 1;
    }
    SDL_Window* window = nullptr;
    SDL_Renderer* renderer = nullptr;
    if (!SDL_CreateWindowAndRenderer("Raspberry Pi OMT Deployer", 1080, 760,
                                     SDL_WINDOW_RESIZABLE, &window, &renderer)) {
        SDL_Quit();
        return 1;
    }
    IMGUI_CHECKVERSION();
    ImGui::CreateContext();
    ImGui::StyleColorsDark();
    ImGui::GetIO().IniFilename = nullptr;
    ImGui_ImplSDL3_InitForSDLRenderer(window, renderer);
    ImGui_ImplSDLRenderer3_Init(renderer);
    Application application;
    bool running = true;
    while (running) {
        SDL_Event event;
        while (SDL_PollEvent(&event)) {
            ImGui_ImplSDL3_ProcessEvent(&event);
            if (event.type == SDL_EVENT_QUIT ||
                (event.type == SDL_EVENT_WINDOW_CLOSE_REQUESTED &&
                 event.window.windowID == SDL_GetWindowID(window))) {
                running = false;
            }
        }
        ImGui_ImplSDLRenderer3_NewFrame();
        ImGui_ImplSDL3_NewFrame();
        ImGui::NewFrame();
        application.draw();
        ImGui::Render();
        SDL_SetRenderDrawColor(renderer, 15, 23, 42, 255);
        SDL_RenderClear(renderer);
        ImGui_ImplSDLRenderer3_RenderDrawData(ImGui::GetDrawData(), renderer);
        SDL_RenderPresent(renderer);
    }
    ImGui_ImplSDLRenderer3_Shutdown();
    ImGui_ImplSDL3_Shutdown();
    ImGui::DestroyContext();
    SDL_DestroyRenderer(renderer);
    SDL_DestroyWindow(window);
    SDL_Quit();
    return 0;
}
