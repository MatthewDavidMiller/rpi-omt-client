add_library(omt_wire STATIC omt_wire.c)
target_include_directories(omt_wire PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_link_libraries(omt_wire PUBLIC omt_warnings omt_hardening omt_sanitizers)
set_target_properties(omt_wire PROPERTIES C_VISIBILITY_PRESET hidden)
