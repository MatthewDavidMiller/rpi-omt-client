find_package(PkgConfig REQUIRED)
pkg_check_modules(ALSA REQUIRED IMPORTED_TARGET alsa)
pkg_check_modules(AVAHI REQUIRED IMPORTED_TARGET avahi-client)
pkg_check_modules(DRM REQUIRED IMPORTED_TARGET libdrm)

set(VMX_SOURCES
    ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/thread_tasks.c
    ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec.c
)
if(CMAKE_SYSTEM_PROCESSOR MATCHES "^(aarch64|arm64)$")
    list(APPEND VMX_SOURCES ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec_arm.c)
else()
    list(APPEND VMX_SOURCES
        ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec_x86.c
        ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec_avx2.c
    )
endif()

add_library(vmx_codec STATIC ${VMX_SOURCES})
target_include_directories(vmx_codec PUBLIC ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src)
target_compile_definitions(vmx_codec PRIVATE _GNU_SOURCE)
target_compile_options(vmx_codec PRIVATE
    $<$<C_COMPILER_ID:Clang>:-fdeclspec>
    $<$<AND:$<NOT:$<STREQUAL:${CMAKE_SYSTEM_PROCESSOR},aarch64>>,$<C_COMPILER_ID:GNU,Clang>>:-mavx2;-mlzcnt;-mbmi>
)
set_target_properties(vmx_codec PROPERTIES
    C_VISIBILITY_PRESET hidden
    POSITION_INDEPENDENT_CODE YES
)
target_link_libraries(vmx_codec PRIVATE omt_hardening)

add_executable(omt-receiver
    main.c
    alsa_output.c
    discovery.c
    drm_output.c
    json_text.c
    native_types.c
    omt_channel.c
    playback_status.c
)
target_include_directories(omt-receiver PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_compile_definitions(omt-receiver PRIVATE OMT_CLIENT_VERSION="${OMT_CLIENT_VERSION}")
target_link_libraries(omt-receiver PRIVATE
    omt_wire
    vmx_codec
    PkgConfig::ALSA
    PkgConfig::AVAHI
    PkgConfig::DRM
    m
    pthread
    omt_warnings
    omt_hardening
    omt_sanitizers
)
set_target_properties(omt-receiver PROPERTIES
    C_VISIBILITY_PRESET hidden
    INTERPROCEDURAL_OPTIMIZATION_RELEASE YES
)
install(TARGETS omt-receiver RUNTIME DESTINATION bin)
