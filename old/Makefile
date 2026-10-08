# brain-emu -- the ARM926EJ-S (ARMv5TEJ) core the i.MX28 has, with an MMU, an
# interpreter and a JIT, built as one core plus pluggable components.
#
#   make            build the library, the tools and the test suites
#   make check      build and run everything (per-component suites + corpus)
#   make bench      run the loop benchmark under the interpreter and the JIT
#   make corpus     (re)generate the ARM ISA corpus from the reference model
#   make layout     print where things live
#   make modules    ask the built tool what this build contains
#   make clean
#
# The layout is one line per kind (docs/ARCHITECTURE.md has the long version):
#
#   core/          the foundation: machine, bus, module registry, public headers
#   machines/<id>/ one CPU module (its interpreter, its JIT, its own tests)
#   devices/<id>/  one device module
#   loaders/<id>/  one image format
#   boards/        named machine presets
#   builtin/       the list of what this build contains
#   tools/         command-line tools (a machine may add its own under tools/)
#
# Every source list below is a glob over those directories, so a new component is
# a new directory plus two lines in builtin/builtin.cpp -- the Makefile does not
# name any module, and no module is named anywhere else in the core either.

CXX      ?= c++
CXXSTD   ?= -std=c++17
OPT      ?= -O2
WARN     ?= -Wall -Wextra -Wshadow -Wpointer-arith -Wcast-align -Wwrite-strings
SAN      ?=
EXTRA    ?=
DEFINES  ?=

BUILD    ?= build
OBJ      := $(BUILD)/obj
BIN      := $(BUILD)
PY       ?= python3

# `core/include` has the public interface (whoever embeds this library writes
# `#include "brainemu/machine.h"`); everything else includes by path from the
# repository root (`"machines/arm926/cpu/state.h"`), so an include line says
# exactly where the file lives.
INC      := -Icore/include -I.
CXXFLAGS := $(CXXSTD) $(OPT) $(WARN) $(SAN) $(DEFINES) -fno-strict-aliasing -g \
            -pthread -MMD -MP $(INC) $(EXTRA)
LDFLAGS  ?=
LDLIBS   ?= -lm

LIB_SRCS   := $(sort $(wildcard core/src/*.cpp) \
                     $(wildcard machines/*/*.cpp) \
                     $(wildcard machines/*/cpu/*.cpp) \
                     $(wildcard machines/*/jit/*.cpp) \
                     $(wildcard devices/*/*.cpp) \
                     $(wildcard loaders/*/*.cpp) \
                     $(wildcard boards/*.cpp) \
                     $(wildcard builtin/*.cpp))
# A `tests/` directory is never part of the library, whichever kind it hangs off.
LIB_SRCS   := $(filter-out %/tests/%,$(LIB_SRCS))
TOOL_SRCS  := $(sort $(wildcard tools/*.cpp) $(wildcard machines/*/tools/*.cpp))
TEST_SRCS  := $(sort $(wildcard core/tests/*.cpp) \
                     $(wildcard machines/*/tests/*.cpp) \
                     $(wildcard devices/tests/*.cpp) \
                     $(wildcard loaders/tests/*.cpp) \
                     $(wildcard boards/tests/*.cpp))
PYTEST_SRCS := $(sort $(wildcard machines/*/tests/test_*.py))

LIB        := $(BUILD)/libbrainemu.a
LIB_OBJS   := $(LIB_SRCS:%.cpp=$(OBJ)/%.o)
# One binary per suite/tool, named after its file: `core/tests/test_machine.cpp`
# becomes `build/test_machine`.  The names are unique across the tree.
TEST_BINS  := $(foreach s,$(TEST_SRCS),$(BIN)/$(notdir $(basename $(s))))
TOOLS      := $(foreach s,$(TOOL_SRCS),$(BIN)/$(notdir $(basename $(s))))
DEPS       := $(LIB_OBJS:.o=.d) \
              $(TEST_SRCS:%.cpp=$(OBJ)/%.o) $(TOOL_SRCS:%.cpp=$(OBJ)/%.o)
DEPS       := $(DEPS:.o=.d)

UNAME_M := $(shell uname -m)

.PHONY: all lib tools tests check bench corpus clean layout modules
all: lib tools tests

lib: $(LIB)
tools: $(TOOLS)
tests: $(TEST_BINS)

$(LIB): $(LIB_OBJS)
	@mkdir -p $(dir $@)
	@rm -f $@
	$(AR) rcs $@ $^
	@echo "  AR      $@"

$(OBJ)/%.o: %.cpp
	@mkdir -p $(dir $@)
	@echo "  CXX     $<"
	$(CXX) $(CXXFLAGS) -c $< -o $@

# One binary per test suite and per tool, named after its file (the names are
# unique across the tree by convention: core/tests/test_machine.cpp -> test_machine).
define BIN_RULE
$(BIN)/$(notdir $(basename $(1))): $(OBJ)/$(basename $(1)).o $(LIB)
	@mkdir -p $(dir $$@)
	@echo "  LINK    $$@"
	$$(CXX) $$(CXXFLAGS) $$(LDFLAGS) $$< $$(LIB) -o $$@ $$(LDLIBS)
endef
$(foreach s,$(TEST_SRCS) $(TOOL_SRCS),$(eval $(call BIN_RULE,$(s))))

# ---- checking --------------------------------------------------------------
# Each component's suite tests that component; `check` runs them all and then the
# machine-specific reference tests in Python (the ISA corpus, the shifter, the
# AArch64 backend), which need a golden model and so are not part of the C++ run.
check: tests tools
	@fail=0; \
	for t in $(TEST_BINS); do printf '  RUN   %s\n' $$t; ./$$t || fail=1; done; \
	printf '  RUN   %s --jit (corpus)\n' $(BIN)/test_instr; \
	./$(BIN)/test_instr machines/arm926/tests/data/corpus.txt --jit || fail=1; \
	for s in $(PYTEST_SRCS); do printf '  RUN   %s\n' $$s; \
	  BRAIN_REQUIRE_TOOLS=1 $(PY) $$s || fail=1; done; \
	if [ $$fail -ne 0 ]; then echo "FAILED"; exit 1; fi
	@echo "all tests passed"

# The ISA corpus is generated from a golden emulator (Unicorn) so the tests do
# not depend on an ARM cross toolchain.  It exercises the ARM926 module, which is
# the module the corpus is written in.
corpus:
	@mkdir -p machines/arm926/tests/data
	$(PY) machines/arm926/tests/gen_instr_tests.py

bench: $(BIN)/bench
	@echo "== interpreter =="; ./$(BIN)/bench --no-jit
ifneq ($(UNAME_M),)
	@echo "== jit =="; ./$(BIN)/bench --jit || true
endif

layout:
	@echo "core/            the foundation: machine, bus, module registry, C API"
	@echo "                 (its public headers are core/include/brainemu/)"
	@echo "machines/<id>/   one CPU module: the interpreter and, per host, a JIT"
	@echo "devices/<id>/    one device module: a window plus behaviour"
	@echo "loaders/<id>/    one image format"
	@echo "boards/          named machine presets"
	@echo "builtin/         the list of what this build contains"
	@echo "testkit/         what every test suite shares"
	@echo "tools/           command-line tools"
	@echo
	@echo "machines:"; @ls -d machines/*/ | sed 's|machines/|  |; s|/$$||'
	@echo "devices:";  @ls -d devices/*/  | sed 's|devices/|  |; s|/$$||' | grep -v tests
	@echo "loaders:";  @ls -d loaders/*/  | sed 's|loaders/|  |; s|/$$||' | grep -v tests

modules: $(BIN)/brainemu
	@./$(BIN)/brainemu --list-modules

clean:
	rm -rf $(BUILD)

-include $(DEPS)
