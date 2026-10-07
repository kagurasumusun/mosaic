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
modules (`elf32`, `raw`), devices are modules, and `src/modules/toy8/` is a
complete second CPU that exists to keep that claim honest. `docs/MODULES.md` is
the guide to the split.

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

# what CPUs this build contains, and one that is not an ARM
build/brainemu --list-cpus
build/brainemu --cpu toy8 --regs program.t8
```

`--list-cpus` prints the module list with descriptions, `--cpu ID` builds the
machine around another CPU module, and `--cpu-opt K=V` hands a module an option
the generic config does not carry. A `--regs` dump names its registers from the
module's own description, so it works for a CPU whose PC is not register 15.

`--exit-port A` maps a one-page device at `A`: a guest store there ends the run
and the value written becomes the tool's exit status, which is all a test image
needs to report success (see `tests/test_unit.cpp` for the encoding). A guest
that ends with `B .` and no exit port simply runs until `--steps` runs out.

## Tests

`make check` runs:

* `tests/test_instr.cpp` — the ISA corpus: single- and multi-instruction cases,
  register/memory/flag state compared against `tests/data/corpus.txt`. The
  corpus is *generated* from Unicorn (QEMU's ARM core) by
  `tests/gen_instr_tests.py` (`make corpus`), which needs a Python
  environment with `unicorn`, `keystone` and `capstone`; running the tests needs
  none of them. Cases whose expectations depend on an emulator default rather than on
  the architecture are deliberately left out; see the notes in the generator.
* `tests/test_unit.cpp` — the API surface around the core: image loading
  (raw and ELF32), device mapping and one-hook-call-per-access behaviour,
  MMIO above 32 MB with the MMU off, register access with the PC aliases,
  and the counters.
* `tests/test_module.cpp` — the core as a core: the module registry, the second
  CPU module (toy8) driven by the machine and by the C API, a CPU module
  declared inside the test file, monitors, the device adapters, and the option
  and error paths. If this passes, no part of `src/core` needs an ARM to run.
* `tests/test_loader.cpp` — the loaders as modules, over a bare `Bus` with no
  CPU: raw placement, ELF32 acceptance and every refusal, a loader declared
  inside the test, and the all-or-nothing promise (a refused image leaves memory
  untouched).
* `tests/test_jit_x64.cpp` (x86-64 hosts; skips elsewhere) — the emitted host
  code, executed in-process: encoder byte pins, the shadow-buffer ABI (a
  sentinel-filled buffer, so a wrong offset shows up as a clobbered word), a
  sweep over every data-processing opcode and operand shape, random multi-
  instruction blocks, and a block that forces the register allocator to spill.

## Layout

```
include/brainemu/     the public headers: the C API (brainemu.h) and the core's
                      interfaces (cpu/, core/, device/, loader/)
src/core/             the machine: bus, devices, module registry, run loop
src/modules/*/        the modules this build contains
  arm926/             the ARM926EJ-S: ARM/Thumb, CP15 + MMU + TLB, interpreter
  arm926/jit/         its block cache, IR and host backends
  toy8/               a second CPU module: the worked example of the interface
src/loader/           the built-in image formats (elf32, raw)
src/api.cpp           the C API, implemented against the machine
tools/                brainemu (runner), bench (throughput), jit_emit (encoder)
tests/                the corpus generator, corpus-driven and unit tests
reference/            the ARM documents this code is written against
docs/                 ARCHITECTURE.md, MODULES.md, ISA-COVERAGE.md, LICENSE-NOTES.md
LICENSE               the brain-emu License, version 1.0 (see docs/LICENSE-NOTES.md)
```

## Status

* One core, modules: `Machine` (bus + one CPU + devices + run loop) over a module
  registry; `CpuCore` is the whole CPU contract and is only called at slice
  boundaries, so a module's inner loop stays non-virtual and monomorphic. Two CPU
  modules ship (`arm926`, `toy8`) and the loaders (`elf32`, `raw`) are modules
  too. `docs/MODULES.md` documents the contract and how to add a processor.
* ARM and Thumb state, ARMv5TEJ data processing, loads/stores (including the
  halfword, signed, dual and swap families), multiply, CLZ, MRS/MSR, branch and
  coprocessor groups, with the exceptions, banked registers, CP15 c1 (Table
  2-11 bit map, reset value 0x00050078), FCSE, coarse/fine page tables, section
  and page permissions, XN, domains, and the ARM926EJ-S fault-status encoding.
* JIT: two execution paths over one interpreter, covering ARM-state data
  processing for now - all sixteen opcodes, RSC included (a block still ends at
  a branch, a load or a store, which the interpreter then retires - see
  `docs/ISA-COVERAGE.md`). The block layer (hash cache,
  `trans_gen` invalidation, self-modification watch, negative cache for refused
  starts), the IR and the front end are shared; the host picks the code
  generator. On x86-64 the x86-64 backend is compiled in and executes blocks
  natively — `build/bench --jit` runs *faster* than `build/bench`, and
  `tests/test_jit_x64.cpp` diffs emitted blocks against the interpreter
  instruction by instruction. On AArch64 the AArch64 backend is selected and
  checked against a model by `tests/test_jit_arm64.py`; on any other host
  `use_jit` degrades to the interpreter (the counters show `interp N jit 0`).
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
