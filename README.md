# brain-emu

ARM926EJ-S (ARMv5TEJ) core emulator, aimed at `cpu=imx28 (arm926ej-s) (armel)`.

Functional-first: an interpreter with an integrated MMU/CP15 model, a block
cache and translation layer that a JIT backend plugs into, and an instruction
corpus generated from a golden emulator so the semantics are checked rather than
asserted.

## Build

```
make            # library, tools, tests   (build/libbrainemu.a, build/brainemu, build/bench)
make check      # build and run every test
make bench      # interpreter throughput, plus the JIT path if a backend is enabled
make clean
```

The only build requirements are a C++17 compiler and GNU make. `CMakeLists.txt`
mirrors the same source lists for IDEs and CI (`cmake -S . -B build-cmake &&
cmake --build build-cmake && ctest --test-dir build-cmake`).

## Running something

```
# a flat binary, entered in ARM state at 0x8000
build/brainemu --raw --addr 0x8000 --steps 1000 --regs program.bin

# a statically linked armel ELF, with the guest deciding when to stop
build/brainemu --elf --exit-port 0x80000000 --stats prog.elf; echo $?
```

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
* `tests/test_jit_x64.cpp` (x86-64 hosts; skips elsewhere) — the emitted host
  code, executed in-process: encoder byte pins, the shadow-buffer ABI (a
  sentinel-filled buffer, so a wrong offset shows up as a clobbered word), a
  sweep over every data-processing opcode and operand shape, random multi-
  instruction blocks, and a block that forces the register allocator to spill.

## Layout

```
include/brainemu/   public headers: the C API (brainemu.h), bus, types
src/core/           Cpu state, decoders, interpreter, CP15 + MMU + TLB, bus
src/jit/            block cache and driver; host backends plug in here
src/loader/         ELF32 / raw image loading
src/api.cpp         the C API, implemented against the core
tools/              brainemu (runner), bench (throughput)
tests/              the corpus generator, corpus-driven and unit tests
reference/          the ARM documents this code is written against
docs/               ARCHITECTURE.md, ISA-COVERAGE.md, LICENSE-NOTES.md
LICENSE             the brain-emu License, version 1.0 (see docs/LICENSE-NOTES.md)
```

## Status

* ARM and Thumb state, ARMv5TEJ data processing, loads/stores (including the
  halfword, signed, dual and swap families), multiply, CLZ, MRS/MSR, branch and
  coprocessor groups, with the exceptions, banked registers, CP15 c1 (Table
  2-11 bit map, reset value 0x00050078), FCSE, coarse/fine page tables, section
  and page permissions, XN, domains, and the ARM926EJ-S fault-status encoding.
* JIT: two execution paths over one interpreter. The block layer (hash cache,
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

See `docs/ARCHITECTURE.md` for the invariants the core relies on, and
`docs/ISA-COVERAGE.md` for the instruction-by-instruction status.

## License

The brain-emu License, version 1.0 (`SPDX-License-Identifier:
LicenseRef-brain-emu-1.0`) - the project's own license, written for it rather
than adopted: use, modify, redistribute and sell, keep the notices, ship the
text, say what you changed, and no warranty.  The full text is in `LICENSE`, and
`docs/LICENSE-NOTES.md` explains the choices, what the license does *not* cover
(the ARM/NXP manuals in `reference/`, and the development-time tools), and where
the corpus comes from.
