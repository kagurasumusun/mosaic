# brain-emu -- mirrors the Makefile's source lists (see Makefile for options).
cmake_minimum_required(VERSION 3.16)
project(brainemu CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE RelWithDebInfo)
endif()

file(GLOB BRAIN_CORE_SRCS CONFIGURE_DEPENDS src/core/*.cpp src/jit/*.cpp
     src/loader/*.cpp)
add_library(brainemu STATIC ${BRAIN_CORE_SRCS} src/api.cpp)
target_include_directories(brainemu PUBLIC include PRIVATE src)
target_compile_options(brainemu PRIVATE -Wall -Wextra -Wshadow
                       -Wpointer-arith -Wcast-align -Wwrite-strings
                       -fno-strict-aliasing)

# The tool is built from tools/brainemu.cpp but the *target* has to differ from
# the library's name; the produced binary is still called brainemu.
add_executable(brainemu_cli tools/brainemu.cpp)
set_target_properties(brainemu_cli PROPERTIES OUTPUT_NAME brainemu)
target_link_libraries(brainemu_cli PRIVATE brainemu m)

add_executable(bench tools/bench.cpp)
target_link_libraries(bench PRIVATE brainemu m)

enable_testing()
foreach(test test_instr test_unit)
  add_executable(${test} tests/${test}.cpp)
  target_link_libraries(${test} PRIVATE brainemu m)
  add_test(NAME ${test} COMMAND ${test})
endforeach()
