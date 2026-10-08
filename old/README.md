# brain-emu

A CPU emulator for the i.MX28's processor - ARM926EJ-S, `cpu=imx28
(arm926ej-s) (armel)` - built as **one core plus modules**, so a second
processor is a new directory rather than a new emulator.

Functional-first: an interpreter with an integrated MMU/CP15 model, a block
cache and translation layer that a JIT backend plugs into, and an instruction
corpus generated from a golden emulator so the semantics are checked rather than
asserted.

Two execution paths, both live: the interpreter, and a JIT that runs *this*
host's code (x86-64 and AArch64 backends), selected per machine. Nothing about
the core is ARM-specific - the CPU is a module (`arm926`), the loaders are
modules (`elf32`, `raw`), devices and monitors are modules too (`console`,
`exit`, and any the tests declare), and `machines/toy8/` is a complete second
CPU that exists to keep that claim honest. A machine is assembled out of *ids*
(`--cpu`, `--device`, `--monitor`, or the same fields in `MachineConfig` /
`brain_config_t`), so a build can contain a peripheral the core has never heard
of. `docs/MODULES.md` is the guide to the split.

## Build

```
make            # library, tools, tests   (build/libbrainemu.a, build/brainemu, build/bench)
make check      # build and run every test
make bench      # interpreter throughput, plus the JIT path if a backend is enabled
make clean
```

The only build requirements are a C++17 compiler and GNU make. `CMakeLists.txt`
mirrors the same source lists for IDEs and CI (`cmake -S . -B build-cmake &&
cmake --build build-cmake && ctest --test-dir build-cmake`).  A build tree from
before a module moved needs `make clean`: the source lists are globs, and a
stale archive is not rebuilt for a source that no longer exists.

## Running something

```
# a flat binary, entered in ARM state at 0x8000
build/brainemu --raw --addr 0x8000 --steps 1000 --regs program.bin

# a statically linked armel ELF, with the guest deciding when to stop
build/brainemu --elf --exit-port 0x80000000 --stats prog.elf; echo $?

# a bare-metal image that prints and then decides when to stop
build/brainemu --device console --exit prog.bin; echo $?

# a named machine: a board preset, plus a wire of your own
build/brainemu --board timer --wire timer.irq=irq --stats prog.bin

# two interrupt sources behind one controller, on the CPU's one IRQ line
build/brainemu --board dual-timer --stats prog.bin
build/brainemu --device timer@0x10001000#t0 --device timer@0x10001100#t1 \
               --device intc --wire t0.irq=intc.irq0 --wire t1.irq=intc.irq1 \
               --wire intc.irq=irq prog.bin

# what CPUs this build contains, and one that is not an ARM
build/brainemu --list-cpus
build/brainemu --cpu toy8 --regs program.t8
```

`--list-cpus` prints the module list with descriptions, `--cpu ID` builds the
machine around another CPU module, and `--cpu-opt K=V` hands a module an option
the generic config does not carry. A `--regs` dump names its registers from the
module's own description, so it works for a CPU whose PC is not register 15.

`--board ID` starts from a *board* - a named machine preset (which CPU, which
devices, which wires) - and everything else on the command line is applied on top
of it, so `--board timer --device exit` is that board plus a peripheral.
`--wire DEV.OUT=SINK` connects a device's interrupt output to something that has
an input, by name: `--wire timer.irq=irq` a CPU input, `--wire t0.irq=intc.irq0`
a device input, so several sources can share one CPU line through a controller
(as in `--board dual-timer`).  This is the only place two modules are named
together; `--list-modules` prints the boards, devices and monitors this build
contains.

`--device ID[@A]` builds a device module into the machine (at `A`, or at the
address the module says is its default), `--monitor ID` tells a monitor module
about the run, and `--exit` is the shorthand for the `exit` module - a guest
store there ends the run and the value becomes the tool's exit status, which is
all a test image needs to report success. `--exit-port A` is the older spelling
of the same thing for a caller that wants the port somewhere else: it maps a
callback device at `A`. A guest that ends with `B .` and no exit port simply
runs until `--steps` runs out. `--stats` prints what each device and monitor has
to say about itself (`--device console --exit` gives `bytes=4 log=Hi!` and
`code=0x00000007 status=7 writes=1`), which is how a peripheral the tool has
never heard of still reports in.

## Tests

`make check` runs:

* `machines/arm926/tests/test_instr.cpp` — the ISA corpus: single- and multi-instruction cases,
  register/memory/flag state compared against `machines/arm926/tests/data/corpus.txt`. The
  corpus is *generated* from Unicorn (QEMU's ARM core) by
  `machines/arm926/tests/gen_instr_tests.py` (`make corpus`), which needs a Python
  environment with `unicorn`, `keystone` and `capstone`; running the tests needs
  none of them. Cases whose expectations depend on an emulator default rather than on
  the architecture are deliberately left out; see the notes in the generator.
* `machines/arm926/tests/test_arm926.cpp` — the API surface around the core: image loading
  (raw and ELF32), device mapping and one-hook-call-per-access behaviour,
  MMIO above 32 MB with the MMU off, register access with the PC aliases,
  and the counters.
* `core/tests/test_machine.cpp` — the core as a core: the module registry, the second
  CPU module (toy8) driven by the machine and by the C API, a CPU module
  declared inside the test file, monitors, the device adapters, the device and
  monitor *modules* (built by id, sharing a page, describing themselves), and the
  option and error paths. If this passes, no part of `core/` needs an ARM to
  run.
* `loaders/tests/test_loader.cpp` — the loaders as modules, over a bare `Bus` with no
  CPU: raw placement, ELF32 acceptance and every refusal, a loader declared
  inside the test, and the all-or-nothing promise (a refused image leaves memory
  untouched).
* `machines/arm926/tests/test_jit_x64.cpp` (x86-64 hosts; skips elsewhere) — the emitted host
  code, executed in-process: encoder byte pins, the shadow-buffer ABI (a
  sentinel-filled buffer, so a wrong offset shows up as a clobbered word), a
  sweep over every data-processing opcode and operand shape, a load/store sweep
  over the addresses that decide a bail, every branch condition against every
  flag word, a Thumb program list with its own branch sweep and the state
  transitions in and out of Thumb, the dispatcher rules that gate translated
  accesses, random multi-instruction blocks, and a block that forces the register
  allocator to spill.  Registers, R13/R14, CPSR (including the instruction-set
  bit), the resume pc, the cycle count and RAM are all diffed against the
  interpreter.

## Layout

```
core/                 the foundation: no CPU, no device, no target in it
  include/brainemu/   the public interface (brainemu.h, machine.h, module.h,
                      bus.h, cpu.h, device.h, monitor.h, loader.h, types.h)
  src/                the machine, the bus, the module registry, the C API
  tests/              the core's own suite (it names no machine)
machines/arm926/      the primary target: ARM926EJ-S (ARMv5TEJ)
  cpu/                the interpreter: decode (ARM + Thumb), CP15, MMU/TLB, ops
  jit/                its block cache, IR and host backends (x86-64, AArch64)
  tests/              its suites (unit, corpus, MMU, JIT, AArch64) + the corpus
  tools/              jit_emit: translate one word and print what came out
machines/toy8/        a second CPU module: the worked example of the interface
devices/console/      a device module: bytes written there become text
devices/exit/         a device module: a store there stops the machine
devices/timer/        a device module with an interrupt output (the reference)
devices/intc/         a device with interrupt *inputs*: a controller
loaders/elf32/        one image format per directory (elf32, raw)
loaders/raw/
boards/               named machine presets (minimal, timer, dual-timer)
builtin/              the list of what this build contains
testkit/              what every test suite shares
tools/                brainemu (runner), bench (throughput)
reference/            the ARM documents this code is written against
docs/                 ARCHITECTURE.md, MODULES.md, ISA-COVERAGE.md, LICENSE-NOTES.md
LICENSE               the brain-emu License, version 1.0 (see docs/LICENSE-NOTES.md)
```

The rule is one line long and holds everywhere: **`core/` is the foundation, and
everything else is a component of exactly one kind** - a machine (`machines/<id>/`),
a device (`devices/<id>/`), a loader (`loaders/<id>/`) or a board (`boards/`).  A
component's directory holds its sources, its header with the one function that
registers it, and (if it has tests) a `tests/` directory next to them.  The build
globs those directories and `builtin/builtin.cpp` calls their registration
functions, so adding a component is a directory plus two lines there - and nothing
else in the tree has to change, because nothing else names it.

## Status

* One core, modules: `Machine` (bus + one CPU + devices + run loop) over a module
  registry; `CpuCore` is the whole CPU contract and is only called at slice
  boundaries, so a module's inner loop stays non-virtual and monomorphic. Five
  kinds of module ship - CPUs (`arm926`, `toy8`), loaders (`elf32`, `raw`),
  devices (the console, the exit port, a timer with an interrupt output, and an
  interrupt controller with inputs), monitors, and *boards* (named machine
  presets: which CPU, which devices, how their interrupt lines are wired). A
  device declares interrupt *outputs*; a device or a CPU declares named interrupt
  *inputs*; a board is the only place that names both ends, and the machine
  carries the level between them, so no module knows another module's type. Two
  devices built from one module are told apart by an instance name
  (`--device timer@0x10001000#t0`), which is what a wire names. Each reports its state through name/value fields the core only
  forwards. `docs/MODULES.md` documents the contract and how to add a processor,
  a device, a monitor or a board.
* ARM and Thumb state, ARMv5TEJ data processing, loads/stores (including the
  halfword, signed, dual and swap families), multiply, CLZ, MRS/MSR, branch and
  coprocessor groups, with the exceptions, banked registers, CP15 c1 (Table
  2-11 bit map, reset value 0x00050078), FCSE, coarse/fine page tables, section
  and page permissions, XN, domains, and the ARM926EJ-S fault-status encoding.
* JIT: two execution paths over one interpreter, in both instruction sets.  A
  block is data processing (all sixteen ARM opcodes, RSC included), the LDR/STR
  families - a block performs its accesses inline, guarded so that anything
  outside what it can do *bails* back to the interpreter, which owns the abort
  and self-modification paths - and one terminating branch, conditional or not,
  with the condition evaluated in the block and a `bx`/`blx` able to change the
  instruction set.  The front end decodes ARM words or Thumb halfwords depending
  on the state the dispatcher hands it, so the same IR, the same emitters and the
  same exit protocol cover both; see `docs/ISA-COVERAGE.md` for the exact set and
  `docs/ARCHITECTURE.md` for the exit protocol.  The block layer (hash cache,
  `trans_gen` invalidation, self-modification watch, negative cache for refused
  starts, and the instruction-set bit in a block's identity), the IR and the
  front end are shared; the host picks the code generator.  On x86-64 the x86-64
  backend is compiled in and executes blocks natively: `build/bench --jit` runs
  the counted add/cmp/branch loop at ~130 MInsn/s against ~74 for the
  interpreter, and the same loop in Thumb at ~126 against ~53, because the loop
  is now one block instead of three instructions and an interpreted branch.  On
  AArch64 the AArch64 backend is selected (data processing only) and checked
  against a model by `machines/arm926/tests/test_jit_arm64.py`; on any other host `use_jit`
  degrades to the interpreter (the counters show `interp N jit 0`).
* Not implemented on purpose: Jazelle, Vectored FIQ, TCM, cache-line semantics,
  32-bit Thumb encodings (ARMv6+), `MCR2/MRC2` (ARMv7), `IT`, `CBZ/CBB`,
  `MOVW/MOVT`, `REV/SXTH`. These decode as undefined instructions.
* No operating-system layer: an ELF's segments are placed in physical memory and
  a stack pointer is provided, but there are no Linux system calls.

See `docs/ARCHITECTURE.md` for the invariants the core relies on,
`docs/MODULES.md` for the module system, and `docs/ISA-COVERAGE.md` for the
instruction-by-instruction status.

## License

The brain-emu License, version 1.0 (`SPDX-License-Identifier:
LicenseRef-brain-emu-1.0`) - the project's own license, written for it rather
than adopted: use, modify, redistribute and sell, keep the notices, ship the
text, say what you changed, and no warranty.  The full text is in `LICENSE`, and
`docs/LICENSE-NOTES.md` explains the choices, what the license does *not* cover
(the ARM/NXP manuals in `reference/`, and the development-time tools), and where
the corpus comes from.
