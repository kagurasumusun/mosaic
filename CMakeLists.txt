# brain-emu -- mirrors the Makefile's source lists (see the Makefile for options).
#
# The Makefile is the primary build; this file exists so an IDE or a CI job that
# speaks CMake can build the tree without duplicating the layout rules.  The
# layout itself is documented in docs/ARCHITECTURE.md: `core/` is the foundation,
# and every other directory is a component of one kind (machines, devices,
# loaders, boards) whose sources are globbed, exactly as in the Makefile.
cmake_minimum_required(VERSION 3.16)
project(brainemu CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE RelWithDebInfo)
endif()

# One `file(GLOB ...)` per component kind, and a filter for the suites: a `tests`
# directory is never part of the library, whichever kind it hangs off.
file(GLOB_RECURSE BRAIN_CORE_SRCS CONFIGURE_DEPENDS
     core/src/*.cpp machines/*/*.cpp machines/*/cpu/*.cpp machines/*/jit/*.cpp
     devices/*/*.cpp loaders/*/*.cpp boards/*.cpp builtin/*.cpp)
list(FILTER BRAIN_CORE_SRCS EXCLUDE REGEX "/tests/")

add_library(brainemu STATIC ${BRAIN_CORE_SRCS})
# Both include roots, and *public*: the library itself includes its own headers
# as "brainemu/x.h" and everything else by repo-root path ("devices/exit/exit.h"),
# and so do the suites and tools that link it - they include the shared test
# helpers as "testkit/test_support.h".  Same two -I flags as the Makefile.
target_include_directories(brainemu PUBLIC core/include .)
target_compile_options(brainemu PRIVATE -Wall -Wextra -Wshadow
                       -Wpointer-arith -Wcast-align -Wwrite-strings
                       -fno-strict-aliasing)

# The tool's source is tools/brainemu.cpp, but the *target* has to differ from the
# library's name; the produced binary is still called brainemu.
add_executable(brainemu_cli tools/brainemu.cpp)
set_target_properties(brainemu_cli PROPERTIES OUTPUT_NAME brainemu)
target_link_libraries(brainemu_cli PRIVATE brainemu m)

add_executable(bench tools/bench.cpp)
target_link_libraries(bench PRIVATE brainemu m)

# A machine may bring its own tools; they are built like any other.
file(GLOB BRAIN_MACHINE_TOOLS CONFIGURE_DEPENDS machines/*/tools/*.cpp)
foreach(tool ${BRAIN_MACHINE_TOOLS})
  get_filename_component(tool_name ${tool} NAME_WE)
  add_executable(${tool_name} ${tool})
  target_link_libraries(${tool_name} PRIVATE brainemu m)
endforeach()

# One test binary per suite, wherever the suite lives.
enable_testing()
file(GLOB BRAIN_TEST_SRCS CONFIGURE_DEPENDS
     core/tests/*.cpp machines/*/tests/*.cpp devices/tests/*.cpp
     loaders/tests/*.cpp boards/tests/*.cpp)
foreach(src ${BRAIN_TEST_SRCS})
  get_filename_component(suite ${src} NAME_WE)
  add_executable(${suite} ${src})
  target_link_libraries(${suite} PRIVATE brainemu m)
  add_test(NAME ${suite} COMMAND ${suite})
endforeach()
