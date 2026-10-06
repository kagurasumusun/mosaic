# brain-emu -- ARM926EJ-S (ARMv5TEJ) core with an integrated MMU.
#
# The Makefile is the primary build (no generated build system needed to hack
# on the core); CMakeLists.txt mirrors it for IDEs and CI.
#
#   make            build the library, the tool and the tests
#   make check      build and run everything (unit + ISA corpus + MMU)
#   make bench      run the loop benchmark under the interpreter and JIT
#   make corpus     (re)generate the ISA corpus from Unicorn via Python
#   make clean

CXX      ?= c++
CXXSTD   ?= -std=c++17
OPT      ?= -O2
WARN     ?= -Wall -Wextra -Wshadow -Wpointer-arith -Wcast-align -Wwrite-strings
SAN      ?=
EXTRA    ?=
DEFINES  ?=

BUILD    ?= build
LIBDIR   := $(BUILD)/obj
BIN      := $(BUILD)

INC      := -Iinclude -Isrc
CXXFLAGS := $(CXXSTD) $(OPT) $(WARN) $(SAN) $(DEFINES) -fno-strict-aliasing -g \
            -pthread -MMD -MP $(INC) $(EXTRA)
LDFLAGS  ?=
LDLIBS   ?= -lm

# sources: only what exists, so the tree builds while the JIT/loader land
CORE_SRCS  := $(sort $(wildcard src/core/*.cpp) $(wildcard src/jit/*.cpp) \
                     $(wildcard src/loader/*.cpp) $(wildcard src/api.cpp))
TOOL_SRCS  := $(wildcard tools/*.cpp)
TEST_SRCS  := $(wildcard tests/test_*.cpp)

CORE_OBJS  := $(CORE_SRCS:%.cpp=$(LIBDIR)/%.o)
LIB        := $(BUILD)/libbrainemu.a
TOOLS      := $(TOOL_SRCS:tools/%.cpp=$(BIN)/%)
TESTBINS   := $(TEST_SRCS:tests/%.cpp=$(BIN)/%)
DEPS       := $(CORE_OBJS:.o=.d) $(TEST_SRCS:tests/%.cpp=$(LIBDIR)/tests/%.o:.o=.d)

UNAME_M := $(shell uname -m)
PY ?= python3

.PHONY: all check clean tools tests bench corpus lib
all: lib tools tests

lib: $(LIB)
tools: $(TOOLS)
tests: $(TESTBINS)

$(LIB): $(CORE_OBJS)
	@mkdir -p $(dir $@)
	@rm -f $@
	$(AR) rcs $@ $^
	@echo "  AR      $@"

$(LIBDIR)/%.o: %.cpp
	@mkdir -p $(dir $@)
	@echo "  CXX     $<"
	$(CXX) $(CXXFLAGS) -c $< -o $@

$(LIBDIR)/tests/%.o: tests/%.cpp
	@mkdir -p $(dir $@)
	@echo "  CXX     $<"
	$(CXX) $(CXXFLAGS) -c $< -o $@

$(BIN)/%: $(LIBDIR)/tests/%.o $(LIB)
	@mkdir -p $(dir $@)
	@echo "  LINK    $@"
	$(CXX) $(CXXFLAGS) $(LDFLAGS) $< $(LIB) -o $@ $(LDLIBS)

$(BIN)/%: tools/%.cpp $(LIB)
	@mkdir -p $(dir $@)
	@echo "  LINK    $@"
	$(CXX) $(CXXFLAGS) $(LDFLAGS) $< $(LIB) -o $@ $(LDLIBS)

# ---- tests ---------------------------------------------------------------
check: tests
	@fail=0; for t in $(TESTBINS); do \
	  printf '  RUN   %s\n' $$t; \
	  ./$$t || fail=1; \
	done; \
	printf '  RUN   build/test_instr --jit\n'; \
	./$(BUILD)/test_instr tests/data/corpus.txt --jit || fail=1; \
	printf '  RUN   tests/test_jit_arm64.py\n'; \
	$(PY) tests/test_jit_arm64.py || fail=1; \
	if [ $$fail -ne 0 ]; then echo "FAILED"; exit 1; fi
	@echo "all tests passed"

# The ISA corpus is generated from a golden emulator (Unicorn) so the tests
# do not depend on an ARM cross toolchain being installed.
corpus:
	@mkdir -p tests/data
	$(PY) tests/gen_instr_tests.py


bench: $(BIN)/bench
	@echo "== interpreter =="; ./$(BIN)/bench --no-jit
ifneq ($(UNAME_M),)
	@echo "== jit =="; ./$(BIN)/bench --jit || true
endif

clean:
	rm -rf $(BUILD)

-include $(DEPS)
