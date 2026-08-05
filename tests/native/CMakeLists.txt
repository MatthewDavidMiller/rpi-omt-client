function(omt_enable_test_assertions target)
    if(MSVC)
        target_compile_options(${target} PRIVATE /UNDEBUG)
    else()
        target_compile_options(${target} PRIVATE -UNDEBUG)
    endif()
endfunction()

find_package(Python3 REQUIRED COMPONENTS Interpreter)

# The wire suites exist wherever the appliance code does. A Windows deployer
# cross build carries no receiver, so it contributes only the deployer suites.
if(TARGET omt_wire)
    add_executable(test_omt_wire test_omt_wire.c)
    target_link_libraries(test_omt_wire PRIVATE omt_wire omt_warnings omt_sanitizers)
    omt_enable_test_assertions(test_omt_wire)
    add_test(NAME native-wire-contract COMMAND test_omt_wire)

    add_executable(test_omt_validation_probe validation_probe.c)
    target_link_libraries(test_omt_validation_probe PRIVATE omt_wire omt_warnings omt_sanitizers)
    omt_enable_test_assertions(test_omt_validation_probe)
    add_test(
        NAME native-validation-vectors
        COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/test_validation_vectors.py
                $<TARGET_FILE:test_omt_validation_probe>
                ${PROJECT_SOURCE_DIR}/tests/schema/omt-target-vectors.json)
endif()

if(UNIX)
    add_executable(test_playback_status
        test_playback_status.cpp
        ${PROJECT_SOURCE_DIR}/src/native/receiver/playback_status.cpp
    )
    target_include_directories(test_playback_status PRIVATE ${PROJECT_SOURCE_DIR}/src/native/receiver)
    target_link_libraries(test_playback_status PRIVATE omt_wire omt_warnings omt_sanitizers pthread)
    omt_enable_test_assertions(test_playback_status)
    add_test(NAME native-playback-status COMMAND test_playback_status)

    add_executable(test_status_projection_probe
        status_projection_probe.cpp
        ${PROJECT_SOURCE_DIR}/src/native/receiver/playback_status.cpp)
    target_include_directories(test_status_projection_probe PRIVATE
        ${PROJECT_SOURCE_DIR}/src/native/receiver)
    target_link_libraries(test_status_projection_probe PRIVATE
        omt_wire omt_warnings omt_sanitizers pthread)
    omt_enable_test_assertions(test_status_projection_probe)
    add_test(
        NAME native-playback-status-vectors
        COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/test_status_vectors.py
                $<TARGET_FILE:test_status_projection_probe>
                ${PROJECT_SOURCE_DIR}/tests/schema/playback-status-vectors.json)
endif()

if(TARGET omt-receiver)
    add_test(
        NAME native-receiver-cli
        COMMAND ${CMAKE_CURRENT_SOURCE_DIR}/test_receiver_cli.sh $<TARGET_FILE:omt-receiver>
    )
    add_test(
        NAME native-discovery-server
        COMMAND ${Python3_EXECUTABLE} ${CMAKE_CURRENT_SOURCE_DIR}/test_discovery_server.py
                $<TARGET_FILE:omt-receiver>
    )
endif()

if(TARGET omt_deployer_core)
    add_executable(test_deployer_core test_deployer_core.cpp)
    target_link_libraries(test_deployer_core PRIVATE omt_deployer_core omt_warnings omt_sanitizers)
    omt_enable_test_assertions(test_deployer_core)
    add_test(NAME native-deployer-core COMMAND test_deployer_core)
endif()
