# brain-emu -- modular system emulator.
#
# Layout summary:
#   core/       emulator foundation
#   arch/       guest architecture, ISA and CPU implementations
#   soc/        SoC specifications/implementations
#   machine/    reserved for concrete machine composition when needed
#   boards/     named board/machine presets
#   devices/    reusable memory-mapped peripherals
#   buses/      reusable interconnect implementations
#   loaders/    guest image formats
#   host/       host-side I/O and presentation
#   debugger/   debugging/inspection services
#   tools/      command-line tools
#   tests/      cross-component tests; component suites stay with components
#   reference/  vendor manuals and architectural references
#   old/        archived legacy tree; deliberately untouched

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

INC      := -Icore/include -I.
CXXFLAGS := $(CXXSTD) $(OPT) $(WARN) $(SAN) $(DEFINES) -fno-strict-aliasing -g \
            -pthread -MMD -MP $(INC) $(EXTRA)
LDFLAGS  ?=
LDLIBS   ?= -lm

# Recursive wildcard helper: unlike the historical layout, component depth is
# not part of the build contract, so adding e.g. devices/interrupt/gic/ is enough.
rwildcard = $(foreach d,$(wildcard $(1)*/),$(call rwildcard,$(d),$(2)) $(wildcard $(d)$(2)))

ALL_CPP_SRCS := $(sort $(wildcard core/src/*.cpp) \
                     $(call rwildcard,arch/,*.cpp) \
                     $(call rwildcard,devices/,*.cpp) \
                     $(call rwildcard,loaders/,*.cpp) \
                     $(call rwildcard,boards/,*.cpp) \
                     $(call rwildcard,host/,*.cpp) \
                     $(call rwildcard,debugger/,*.cpp) \
                     $(wildcard builtin/*.cpp))
TOOL_SRCS  := $(sort $(wildcard tools/*.cpp) $(call rwildcard,arch/,tools/*.cpp))
TEST_SRCS  := $(sort $(wildcard core/tests/*.cpp) \
                     $(call rwildcard,arch/,test_*.cpp) \
                     $(call rwildcard,devices/,test_*.cpp) \
                     $(call rwildcard,loaders/,test_*.cpp) \
                     $(call rwildcard,boards/,test_*.cpp) \
                     $(call rwildcard,host/,test_*.cpp) \
                     $(call rwildcard,debugger/,test_*.cpp) \
                     $(call rwildcard,tests/,test_*.cpp))
LIB_SRCS   := $(filter-out $(TEST_SRCS),$(ALL_CPP_SRCS))
PYTEST_SRCS := $(sort $(call rwildcard,arch/,test_*.py))

LIB        := $(BUILD)/libbrainemu.a
LIB_OBJS   := $(LIB_SRCS:%.cpp=$(OBJ)/%.o)
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

define BIN_RULE
$(BIN)/$(notdir $(basename $(1))): $(OBJ)/$(basename $(1)).o $(LIB)
	@mkdir -p $(dir $$@)
	@echo "  LINK    $$@"
	$$(CXX) $$(CXXFLAGS) $$(LDFLAGS) $$< $$(LIB) -o $$@ $$(LDLIBS)
endef
$(foreach s,$(TEST_SRCS) $(TOOL_SRCS),$(eval $(call BIN_RULE,$(s))))

check: tests tools
	@fail=0; \
	for t in $(TEST_BINS); do printf '  RUN   %s\n' $$t; ./$$t || fail=1; done; \
	if [ -x ./$(BIN)/test_instr ]; then printf '  RUN   %s --jit (corpus)\n' $(BIN)/test_instr; \
	  ./$(BIN)/test_instr arch/arm/cpu/classic/arm9/arm926ej-s/tests/data/corpus.txt --jit || fail=1; fi; \
	for s in $(PYTEST_SRCS); do printf '  RUN   %s\n' $$s; \
	  BRAIN_REQUIRE_TOOLS=1 $(PY) $$s || fail=1; done; \
	if [ $$fail -ne 0 ]; then echo "FAILED"; exit 1; fi
	@echo "all tests passed"

corpus:
	@mkdir -p arch/arm/cpu/classic/arm9/arm926ej-s/tests/data
	$(PY) arch/arm/cpu/classic/arm9/arm926ej-s/tests/gen_instr_tests.py

bench: $(BIN)/bench
	@echo "== interpreter =="; ./$(BIN)/bench --no-jit
ifneq ($(UNAME_M),)
	@echo "== jit =="; ./$(BIN)/bench --jit || true
endif

layout:
	@echo "core/             emulator foundation"
	@echo "arch/             guest ISA, CPU cores, and architecture-specific JITs"
	@echo "soc/              SoC implementations and register maps"
	@echo "boards/           named machine/board presets"
	@echo "devices/          reusable peripherals"
	@echo "buses/             reusable interconnects"
	@echo "loaders/           guest image formats"
	@echo "host/              host-side services"
	@echo "debugger/          debug and tracing services"
	@echo "reference/         architectural/vendor manuals"
	@echo "old/               untouched legacy archive"

modules: $(BIN)/brainemu
	@./$(BIN)/brainemu --list-modules

clean:
	rm -rf $(BUILD)

-include $(DEPS)
