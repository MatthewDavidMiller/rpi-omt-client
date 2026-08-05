include(FetchContent)

# Release archives and hashes are intentionally centralized. Configure may use
# FETCHCONTENT_SOURCE_DIR_* mirrors for offline builds; otherwise CMake rejects
# any archive whose digest differs from this lock.
set(OMT_SDL3_URL
    "https://github.com/libsdl-org/SDL/releases/download/release-3.4.8/SDL3-3.4.8.tar.gz")
set(OMT_SDL3_SHA256
    "e9fff7467fb60f037e6708da18b25560649e4c63edc2a69bb871b960d9cbfbba")
set(OMT_IMGUI_URL
    "https://github.com/ocornut/imgui/archive/refs/tags/v1.92.8.tar.gz")
set(OMT_IMGUI_SHA256
    "fecb33d33930e12ff53a34064e9d3a06c8f7c3e04408f14cd36c80e3faac863b")
set(OMT_LIBSSH2_URL
    "https://libssh2.org/download/libssh2-1.11.1.tar.gz")
set(OMT_LIBSSH2_SHA256
    "d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7")

# Present a fetched dependency's headers to our targets as system headers, so
# consumers keep -Werror without inheriting upstream warnings.
function(omt_mark_includes_system target)
    if(NOT TARGET ${target})
        return()
    endif()
    get_target_property(include_directories ${target} INTERFACE_INCLUDE_DIRECTORIES)
    if(include_directories)
        set_property(TARGET ${target} APPEND
            PROPERTY INTERFACE_SYSTEM_INCLUDE_DIRECTORIES ${include_directories})
    endif()
endfunction()

function(omt_fetch_deployer_dependencies)
    set(CMAKE_EXPORT_NO_PACKAGE_REGISTRY ON)
    set(SDL_SHARED OFF CACHE BOOL "" FORCE)
    set(SDL_STATIC ON CACHE BOOL "" FORCE)
    set(SDL_TEST_LIBRARY OFF CACHE BOOL "" FORCE)
    set(SDL_TESTS OFF CACHE BOOL "" FORCE)
    set(SDL_EXAMPLES OFF CACHE BOOL "" FORCE)
    set(SDL_AUDIO OFF CACHE BOOL "" FORCE)
    set(SDL_CAMERA OFF CACHE BOOL "" FORCE)
    set(SDL_GPU OFF CACHE BOOL "" FORCE)
    set(SDL_HAPTIC OFF CACHE BOOL "" FORCE)
    set(SDL_JOYSTICK OFF CACHE BOOL "" FORCE)
    set(SDL_SENSOR OFF CACHE BOOL "" FORCE)
    set(SDL_DIALOG OFF CACHE BOOL "" FORCE)
    set(SDL_HIDAPI OFF CACHE BOOL "" FORCE)
    set(SDL_POWER OFF CACHE BOOL "" FORCE)
    set(SDL_TRAY OFF CACHE BOOL "" FORCE)
    set(SDL_DBUS OFF CACHE BOOL "" FORCE)
    set(SDL_IBUS OFF CACHE BOOL "" FORCE)
    set(SDL_OPENGL OFF CACHE BOOL "" FORCE)
    set(SDL_OPENGLES OFF CACHE BOOL "" FORCE)
    set(SDL_VULKAN OFF CACHE BOOL "" FORCE)
    set(SDL_INSTALL OFF CACHE BOOL "" FORCE)
    if(UNIX AND NOT APPLE)
        set(SDL_WAYLAND OFF CACHE BOOL "" FORCE)
        set(SDL_X11 ON CACHE BOOL "" FORCE)
        foreach(feature XCURSOR XDBE XFIXES XINPUT XRANDR XSCRNSAVER XSHAPE XSYNC XTEST)
            set(SDL_X11_${feature} OFF CACHE BOOL "" FORCE)
        endforeach()
    endif()
    FetchContent_Declare(sdl3
        URL "${OMT_SDL3_URL}"
        URL_HASH "SHA256=${OMT_SDL3_SHA256}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
    FetchContent_MakeAvailable(sdl3)

    set(BUILD_STATIC_LIBS ON CACHE BOOL "" FORCE)
    set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
    set(BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
    set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
    set(ENABLE_ZLIB_COMPRESSION OFF CACHE BOOL "" FORCE)
    set(ENABLE_DEBUG_LOGGING OFF CACHE BOOL "" FORCE)
    if(WIN32)
        set(CRYPTO_BACKEND WinCNG CACHE STRING "" FORCE)
    else()
        set(CRYPTO_BACKEND OpenSSL CACHE STRING "" FORCE)
    endif()
    FetchContent_Declare(libssh2
        URL "${OMT_LIBSSH2_URL}"
        URL_HASH "SHA256=${OMT_LIBSSH2_SHA256}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
    FetchContent_MakeAvailable(libssh2)
    if(UNIX AND TARGET libssh2_static)
        target_compile_definitions(libssh2_static PRIVATE _DEFAULT_SOURCE)
    endif()

    FetchContent_Declare(imgui
        URL "${OMT_IMGUI_URL}"
        URL_HASH "SHA256=${OMT_IMGUI_SHA256}"
        DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
    if(POLICY CMP0169)
        cmake_policy(SET CMP0169 OLD)
    endif()
    FetchContent_GetProperties(imgui)
    if(NOT imgui_POPULATED)
        FetchContent_Populate(imgui)
    endif()
    add_library(omt_imgui STATIC
        "${imgui_SOURCE_DIR}/imgui.cpp"
        "${imgui_SOURCE_DIR}/imgui_draw.cpp"
        "${imgui_SOURCE_DIR}/imgui_tables.cpp"
        "${imgui_SOURCE_DIR}/imgui_widgets.cpp"
        "${imgui_SOURCE_DIR}/backends/imgui_impl_sdl3.cpp"
        "${imgui_SOURCE_DIR}/backends/imgui_impl_sdlrenderer3.cpp")
    # SYSTEM: the deployer compiles with -Werror and a strict warning set that
    # describes our code, not upstream's. Without this, a vendored header's
    # style decides whether a locked dependency version builds at all.
    target_include_directories(omt_imgui SYSTEM PUBLIC
        "${imgui_SOURCE_DIR}"
        "${imgui_SOURCE_DIR}/backends")
    target_link_libraries(omt_imgui PUBLIC SDL3::SDL3-static)
    omt_mark_includes_system(SDL3-static)
    omt_mark_includes_system(libssh2_static)
    set(OMT_IMGUI_LICENSE "${imgui_SOURCE_DIR}/LICENSE.txt" PARENT_SCOPE)
endfunction()
