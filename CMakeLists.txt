# brain-emu -- modular system emulator.
#
# The tree is intentionally organized by responsibility instead of by one
# machine. See docs/ARCHITECTURE.md. The build discovers implementation files
# recursively under architecture, devices, loaders, and boards; tests are kept
# out of the library regardless of where they live.
cmake_minimum_required(VERSION 3.16)
project(brainemu CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE RelWithDebInfo)
endif()

file(GLOB_RECURSE BRAIN_CORE_SRCS CONFIGURE_DEPENDS
     core/src/*.cpp
     arch/*.cpp
     devices/*.cpp
     loaders/*.cpp
     boards/*.cpp
     builtin/*.cpp)
list(FILTER BRAIN_CORE_SRCS EXCLUDE REGEX "/tests/")

add_library(brainemu STATIC ${BRAIN_CORE_SRCS})
target_include_directories(brainemu PUBLIC core/include .)
target_compile_options(brainemu PRIVATE -Wall -Wextra -Wshadow
                       -Wpointer-arith -Wcast-align -Wwrite-strings
                       -fno-strict-aliasing)

add_executable(brainemu_cli tools/brainemu.cpp)

set_target_properties(brainemu_cli PROPERTIES OUTPUT_NAME brainemu)
target_link_libraries(brainemu_cli PRIVATE brainemu m)

add_executable(bench tools/bench.cpp)
target_link_libraries(bench PRIVATE brainemu m)

file(GLOB_RECURSE BRAIN_MACHINE_TOOLS CONFIGURE_DEPENDS arch/*.cpp)
list(FILTER BRAIN_MACHINE_TOOLS INCLUDE REGEX "/tools/[^/]+\\.cpp$")
foreach(tool ${BRAIN_MACHINE_TOOLS})
  get_filename_component(tool_name ${tool} NAME_WE)
  add_executable(${tool_name} ${tool})
  target_link_libraries(${tool_name} PRIVATE brainemu m)
endforeach()

enable_testing()
file(GLOB_RECURSE BRAIN_TEST_SRCS CONFIGURE_DEPENDS
     core/tests/*.cpp
     arch/*.cpp
     devices/*.cpp
     loaders/*.cpp
     boards/*.cpp)
list(FILTER BRAIN_TEST_SRCS INCLUDE REGEX "/tests/[^/]+\\.cpp$")
foreach(src ${BRAIN_TEST_SRCS})
  get_filename_component(suite ${src} NAME_WE)
  add_executable(${suite} ${src})
  target_link_libraries(${suite} PRIVATE brainemu m)
  add_test(NAME ${suite} COMMAND ${suite})
  set_tests_properties(${suite} PROPERTIES WORKING_DIRECTORY ${CMAKE_SOURCE_DIR})
endforeach()
