find_package(PkgConfig REQUIRED)
pkg_check_modules(ALSA REQUIRED IMPORTED_TARGET alsa)
pkg_check_modules(AVAHI REQUIRED IMPORTED_TARGET avahi-client)
pkg_check_modules(DRM REQUIRED IMPORTED_TARGET libdrm)

set(VMX_SOURCES
    ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec.cpp
)
if(CMAKE_SYSTEM_PROCESSOR MATCHES "^(aarch64|arm64)$")
    list(APPEND VMX_SOURCES ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec_arm.cpp)
else()
    list(APPEND VMX_SOURCES
        ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec_x86.cpp
        ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src/vmxcodec_avx2.cpp
    )
endif()

add_library(vmx_codec STATIC ${VMX_SOURCES})
target_include_directories(vmx_codec PUBLIC ${PROJECT_SOURCE_DIR}/third_party/omt/libvmx/src)
target_compile_options(vmx_codec PRIVATE
    $<$<CXX_COMPILER_ID:Clang>:-fdeclspec>
    $<$<AND:$<NOT:$<STREQUAL:${CMAKE_SYSTEM_PROCESSOR},aarch64>>,$<CXX_COMPILER_ID:GNU,Clang>>:-mavx2;-mlzcnt;-mbmi>
)
set_target_properties(vmx_codec PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    POSITION_INDEPENDENT_CODE YES
)
target_link_libraries(vmx_codec PRIVATE omt_hardening)

add_executable(omt-receiver
    main.cpp
    alsa_output.cpp
    discovery.cpp
    drm_output.cpp
    omt_channel.cpp
    playback_status.cpp
)
target_include_directories(omt-receiver PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_compile_definitions(omt-receiver PRIVATE OMT_CLIENT_VERSION="${OMT_CLIENT_VERSION}")
target_link_libraries(omt-receiver PRIVATE
    omt_wire
    vmx_codec
    PkgConfig::ALSA
    PkgConfig::AVAHI
    PkgConfig::DRM
    pthread
    omt_warnings
    omt_hardening
    omt_sanitizers
)
set_target_properties(omt-receiver PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    INTERPROCEDURAL_OPTIMIZATION_RELEASE YES
)
install(TARGETS omt-receiver RUNTIME DESTINATION bin)
