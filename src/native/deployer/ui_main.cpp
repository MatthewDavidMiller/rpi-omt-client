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
#include <cmath>
#include <cstddef>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <future>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <string_view>
#include <system_error>
#include <utility>
#include <vector>

namespace {
// Unscaled window geometry. Every value is multiplied by the display's content
// scale before use, then fitted to the display, so the deployer opens at the
// same apparent size on a 4K desktop as on a 1366x768 laptop panel.
constexpr int preferred_width = 1080;
constexpr int preferred_height = 760;
constexpr int minimum_width = 720;
constexpr int minimum_height = 520;
constexpr float minimum_scale = 1.0F;
constexpr float maximum_scale = 4.0F;

/// The scale the operator's desktop expects content to be drawn at.
///
/// This is deliberately the display content scale and not
/// `SDL_GetWindowDisplayScale`, which also folds in pixel density: the SDL
/// renderer backend already scales geometry by the framebuffer scale, so
/// including density here would apply it twice.
float content_scale(SDL_DisplayID display) {
    const float reported = SDL_GetDisplayContentScale(display);
    return reported > 0.0F ? std::clamp(reported, minimum_scale, maximum_scale) : minimum_scale;
}

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

std::optional<std::string> bounded_file(const std::filesystem::path& path) {
    std::ifstream input(path, std::ios::binary);
    if (!input) {
        return std::nullopt;
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

/// Render one legal document, or the search that failed to find it.
std::string legal_document(const std::vector<std::filesystem::path>& roots,
                           const std::string_view name) {
    std::string section = "===== ";
    section.append(name);
    section += " =====\n";
    const auto located = omt::deployer::locate_resource(roots, name);
    if (!located.empty()) {
        if (const auto text = bounded_file(located)) {
            return section + located.string() + "\n\n" + *text + "\n";
        }
    }
    section += "Not found. Searched:\n";
    for (const auto& root : roots) {
        section += "  " + (root / name).string() + "\n";
    }
    section +=
        "The published package ships this file beside its bin directory. Reinstall it, or point "
        "Project root at the source tree.\n";
    return section;
}

/// Re-derive every style metric and the font size from `scale`.
///
/// ImGui bakes the display scale into absolute pixel values, so a rescale has
/// to start from a fresh style. Scaling the live style would compound the
/// previous factor every time the window moves between displays.
void apply_display_scale(const float scale) {
    ImGuiStyle style;
    ImGui::StyleColorsDark(&style);
    style.ScaleAllSizes(scale);
    style.FontScaleDpi = scale;
    ImGui::GetStyle() = style;
}

/// Scale the preferred window for `display` and fit it in the usable work area.
void initial_window_size(SDL_DisplayID display, const float scale, int& width, int& height) {
    const auto scaled = [scale](const int extent) {
        return std::max(1, static_cast<int>(static_cast<float>(extent) * scale));
    };
    width = scaled(preferred_width);
    height = scaled(preferred_height);
    SDL_Rect bounds{};
    if (SDL_GetDisplayUsableBounds(display, &bounds) && bounds.w > 0 && bounds.h > 0) {
        width = std::clamp(bounds.w * 9 / 10, std::min(scaled(minimum_width), width), width);
        height = std::clamp(bounds.h * 9 / 10, std::min(scaled(minimum_height), height), height);
    }
}

/// Cap an input field so it stays readable instead of spanning a wide window.
/// The em-based bound follows the DPI-scaled font.
template <std::size_t Size>
void field(const char* label, std::array<char, Size>& buffer,
           const ImGuiInputTextFlags flags = ImGuiInputTextFlags_None) {
    ImGui::SetNextItemWidth(
        std::min(ImGui::GetContentRegionAvail().x * 0.6F, ImGui::GetFontSize() * 28.0F));
    ImGui::InputText(label, buffer.data(), buffer.size(), flags);
}

struct Application final {
    explicit Application(std::filesystem::path base_directory)
        : executable_directory(std::move(base_directory)) {
        std::error_code error;
        auto working = std::filesystem::current_path(error);
        if (error) {
            working.clear();
        }
        set_value(project_root,
                  omt::deployer::discover_project_root(executable_directory, working).string());
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
        const ImGuiViewport& viewport = *ImGui::GetMainViewport();
        ImGui::SetNextWindowPos(viewport.WorkPos);
        ImGui::SetNextWindowSize(viewport.WorkSize);
        ImGui::Begin("Raspberry Pi OMT Deployer", nullptr,
                     ImGuiWindowFlags_NoMove | ImGuiWindowFlags_NoResize |
                         ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoTitleBar |
                         ImGuiWindowFlags_NoBringToFrontOnFocus);
        ImGui::TextUnformatted("Raspberry Pi OMT Deployer");
        ImGui::SameLine();
        ImGui::TextDisabled("native %s", OMT_CLIENT_VERSION);
        ImGui::Separator();
        // Reserve the activity log's share of the window before the tabs are
        // drawn. A tab that fills its remaining height -- the About legal text
        // -- would otherwise leave the log with no room at all. The split is
        // proportional and bounded in text lines, so it holds at any display
        // scale instead of starving the forms on a small scaled window.
        const float line = ImGui::GetTextLineHeightWithSpacing();
        const float remaining = ImGui::GetContentRegionAvail().y;
        const float activity = std::clamp(remaining * 0.3F, line * 4.0F, line * 10.0F) +
                               ImGui::GetFrameHeightWithSpacing() * 2.0F;
        const float workspace = std::max(remaining - activity, line * 4.0F);
        if (ImGui::BeginChild("workspace", ImVec2(0.0F, workspace))) {
            if (ImGui::BeginTabBar("sections")) {
                draw_connection(available);
                draw_deployment(available);
                draw_wifi(available);
                draw_about();
                ImGui::EndTabBar();
            }
        }
        ImGui::EndChild();
        draw_activity(available);
        ImGui::End();
    }

    void draw_activity(bool available) {
        ImGui::SeparatorText(available ? "Activity" : "Activity - operation in progress");
        std::string snapshot;
        {
            std::scoped_lock lock(log_mutex);
            snapshot = log;
        }
        ImGui::BeginDisabled(available);
        if (ImGui::Button("Cancel operation")) {
            cancel_requested.store(true);
            append("Cancellation requested; waiting for the current safe boundary.\n");
        }
        ImGui::EndDisabled();
        ImGui::SameLine();
        if (ImGui::Button("Copy log")) {
            ImGui::SetClipboardText(snapshot.c_str());
        }
        ImGui::BeginChild("log", ImVec2(0.0F, 0.0F), ImGuiChildFlags_Borders,
                          ImGuiWindowFlags_HorizontalScrollbar);
        ImGui::TextUnformatted(snapshot.data(), snapshot.data() + snapshot.size());
        // Follow new output only while the operator is already at the end, so
        // scrolling back to read an earlier failure is not undone every frame.
        if (ImGui::GetScrollY() >= ImGui::GetScrollMaxY()) {
            ImGui::SetScrollHereY(1.0F);
        }
        ImGui::EndChild();
    }

    void draw_connection(bool available) {
        if (!ImGui::BeginTabItem("Connection")) {
            return;
        }
        field("Pi host", host);
        field("SSH username", username);
        field("SSH port", port, ImGuiInputTextFlags_CharsDecimal);
        ImGui::Checkbox("Use private key", &use_key);
        if (use_key) {
            field("Private key", key_path);
            field("Key passphrase", key_passphrase, ImGuiInputTextFlags_Password);
        } else {
            field("SSH password", password, ImGuiInputTextFlags_Password);
        }
        field("Sudo password (optional)", sudo_password, ImGuiInputTextFlags_Password);
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
        field("Project root", project_root);
        field("Remote directory", remote_directory);
        ImGui::Checkbox("Build ARM64 image", &build_image);
        ImGui::TextWrapped("The project root is the source tree holding deploy/manifest-v3.txt.");
        // Each snapshot is taken inside its own button, not once per frame:
        // building one copies the SSH, key, and sudo secrets onto the heap.
        ImGui::BeginDisabled(!available);
        if (ImGui::Button("Install prerequisites")) {
            const auto root = options().project_root;
            start([this, root] {
                omt::deployer::DeploymentService service(OMT_CLIENT_VERSION,
                    [this](auto line) { append(line); },
                    [this] { return cancel_requested.load(); });
                service.install_prerequisites(root);
            });
        }
        ImGui::SameLine();
        if (ImGui::Button("Build and deploy")) {
            const auto snapshot_connection = connection();
            const auto snapshot_options = options();
            start([this, snapshot_connection, snapshot_options] {
                omt::deployer::DeploymentService service(OMT_CLIENT_VERSION,
                    [this](auto line) { append(line); },
                    [this] { return cancel_requested.load(); });
                service.deploy(snapshot_connection, snapshot_options);
            });
        }
        draw_management("Status", &omt::deployer::DeploymentService::status);
        ImGui::SameLine();
        draw_management("Recent logs", &omt::deployer::DeploymentService::logs);
        ImGui::SameLine();
        draw_management("Restart service", &omt::deployer::DeploymentService::restart);
        ImGui::EndDisabled();
        ImGui::EndTabItem();
    }

    /// A remote management action reports only its own output, so the button
    /// has to say something even when that output is empty.
    using ManagementAction = std::string (omt::deployer::DeploymentService::*)(
        const omt::deployer::Connection&, std::string_view);

    void draw_management(const char* label, ManagementAction action) {
        if (!ImGui::Button(label)) {
            return;
        }
        const auto snapshot_connection = connection();
        const auto directory = options().remote_directory;
        start([this, snapshot_connection, directory, action, label] {
            omt::deployer::DeploymentService service(
                OMT_CLIENT_VERSION, {}, [this] { return cancel_requested.load(); });
            auto output = (service.*action)(snapshot_connection, directory);
            if (output.empty()) {
                output = "(no output)";
            }
            if (output.back() != '\n') {
                output += '\n';
            }
            append(std::string(label) + ":\n" + output);
        });
    }

    void draw_wifi(bool available) {
        if (!ImGui::BeginTabItem("Wi-Fi")) {
            return;
        }
        field("SSID", wifi_ssid);
        field("Wi-Fi password", wifi_password, ImGuiInputTextFlags_Password);
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
        // The texts ship with the package, so they are found next to the
        // executable first and only then under the operator's project root.
        const auto root = value(project_root);
        if (legal_text.empty() || legal_root != root) {
            const auto roots = omt::deployer::resource_roots(executable_directory, root);
            legal_text = legal_document(roots, "LICENSE") + "\n" +
                         legal_document(roots, "THIRD_PARTY_NOTICES.txt");
            legal_root = root;
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
    std::filesystem::path executable_directory;
    std::string legal_text;
    std::string legal_root;
};
}  // namespace

int main(int, char**) {
    if (!SDL_Init(SDL_INIT_VIDEO)) {
        // A GUI-subsystem binary has no console to print to.
        SDL_ShowSimpleMessageBox(SDL_MESSAGEBOX_ERROR, "Raspberry Pi OMT Deployer",
                                 SDL_GetError(), nullptr);
        return 1;
    }
    const float initial_scale = content_scale(SDL_GetPrimaryDisplay());
    int width = preferred_width;
    int height = preferred_height;
    initial_window_size(SDL_GetPrimaryDisplay(), initial_scale, width, height);
    SDL_Window* window = nullptr;
    SDL_Renderer* renderer = nullptr;
    if (!SDL_CreateWindowAndRenderer("Raspberry Pi OMT Deployer", width, height,
                                     SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY,
                                     &window, &renderer)) {
        SDL_ShowSimpleMessageBox(SDL_MESSAGEBOX_ERROR, "Raspberry Pi OMT Deployer",
                                 SDL_GetError(), nullptr);
        SDL_Quit();
        return 1;
    }
    SDL_SetWindowMinimumSize(
        window,
        std::min(static_cast<int>(static_cast<float>(minimum_width) * initial_scale), width),
        std::min(static_cast<int>(static_cast<float>(minimum_height) * initial_scale), height));
    // Without this the loop redraws as fast as the machine allows and pins a
    // core for the whole deployment.
    SDL_SetRenderVSync(renderer, 1);
    IMGUI_CHECKVERSION();
    ImGui::CreateContext();
    ImGui::GetIO().IniFilename = nullptr;
    ImGui::GetIO().ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard;
    ImGui_ImplSDL3_InitForSDLRenderer(window, renderer);
    ImGui_ImplSDLRenderer3_Init(renderer);
    const char* base_path = SDL_GetBasePath();
    Application application{base_path == nullptr ? std::filesystem::path{}
                                                 : std::filesystem::path(base_path)};
    float applied_scale = 0.0F;
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
        if ((SDL_GetWindowFlags(window) & SDL_WINDOW_MINIMIZED) != 0) {
            SDL_Delay(10);
            continue;
        }
        // Re-read every frame: the window can be dragged to a display whose
        // scale differs from the one it opened on.
        const float scale = content_scale(SDL_GetDisplayForWindow(window));
        if (std::fabs(scale - applied_scale) > 0.01F) {
            apply_display_scale(scale);
            applied_scale = scale;
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
