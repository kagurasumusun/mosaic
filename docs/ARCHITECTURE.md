# Architecture and repository layout

The repository separates four different concepts that are easy to accidentally
mix in an emulator project:

1. **ISA generation** — Armv4, Armv5, Armv6, Armv7, Armv8, Armv9.
2. **Execution state / encoding width** — AArch32 or AArch64 for Arm.
3. **CPU core implementation** — e.g. ARM926EJ-S or Cortex-A53.
4. **SoC / machine composition** — e.g. i.MX28 plus its peripherals and a board.

They are intentionally separate directories. A directory name must describe one
of these concepts, not two of them at once.

## Active tree

```text
core/
  include/brainemu/     public C++/C API and interfaces
  src/                  bus, machine runtime, module registry, C API
  tests/                core-only tests

arch/
  arm/
    execution/
      aarch32/          32-bit Arm execution state (the pipeline step)
      aarch64/          64-bit Arm execution state
    isa/
      armv4/ ... armv9/ architecture generations
      armv5/            the ARMv5TEJ ISA: decoders, execution semantics,
                        PSR/flags, banking, exception model (reusable by any
                        ARMv5TEJ core)
    cpu/
      classic/arm7/     classic Arm7-family cores
      classic/arm9/     classic Arm9-family cores
        arm926ej-s/     the current ARM926EJ-S module (core only: CP15, MMU,
                        run loop, JIT; the ISA lives in isa/armv5/)
      classic/arm11/    classic Arm11-family cores
      cortex-a/         Cortex-A application cores
      cortex-r/         Cortex-R real-time cores
      cortex-m/         Cortex-M microcontroller cores
      neoverse/         Neoverse infrastructure cores
  experimental/
    toy8/               synthetic CPU used to validate the CPU-module seam

soc/
  <vendor>/<soc>/       SoC-level integration: registers, clocks, DMA, IRQs, etc.
  nxp/imx28/             reserved for the i.MX28 implementation

machine/
  <arch>/<system>/      concrete machine/system composition

boards/
  generic/               reusable named presets currently shipped by this tree

buses/
  ahb/ apb/ axi/ pci/ pcie/ usb/  reusable interconnect implementations

devices/
  console/ timer/ interrupt/ system/ uart/ ...   reusable peripherals

loaders/
  elf/elf32/ raw/       guest image loaders

host/
  log/ io/ fs/ monitor/ host-side services (logging, byte I/O, filesystems,
                        monitors); no CPU knowledge, C API or callbacks only

debugger/
  gdb/                  a gdb stub: RSP over TCP or memory, over the C API

testkit/                 common test helpers
tests/                   cross-component/system tests
tools/                   user-facing utilities
builtin/                 explicit registration list for this build
docs/                    project documentation
reference/               manuals organized by architecture/core/SoC
old/                     untouched legacy archive
```

## ARM example: ARM926EJ-S and i.MX28

The relationship is:

```text
arch/arm/isa/armv5/                 the ARMv5TEJ ISA (decoders, semantics, PSR)
        |
        |  executed in
arch/arm/execution/aarch32/     the AArch32 pipeline step (ARM/Thumb)
        |
        |  driven by
        v
arch/arm/cpu/classic/arm9/arm926ej-s/   the ARM926EJ-S core (CP15, MMU, JIT)
        |
        |  CPU core integrated into
        v
soc/nxp/imx28/
        |
        |  concrete hardware/system assembly
        v
machine/arm/<system>/
```

`ARM926EJ-S` is therefore **not** a machine directory and `i.MX28` is **not** a
CPU directory. The ISA directory is not a version label either: `armv5/` holds
the actual instruction-set implementation (an ARMv5TEJ core of any family can
drive it), while the core directory holds only what makes the ARM926EJ-S that
core.

## CPU module boundary

The core exposes a small `CpuCore` contract. An individual CPU model implements
that contract and owns everything required to emulate that CPU accurately:
register state, decoding, execution semantics, MMU/CP15 state where applicable,
and CPU-specific translation/JIT code.

For ARM926EJ-S the module is the *core* layer, and it is layered on the ISA
and execution-state layers rather than containing them:

```text
arch/arm/isa/armv5/                     the ARMv5TEJ ISA (shared, core-free)
  state.{h,cpp}          architectural state: IsaState, banking, PSR rules
  ops.h                  barrel shifter, flag helpers, the cycle model
  decode.{h,cpp}         the ARM-state decoder
  decode_thumb.{h,cpp}   the Thumb-state decoder
  exceptions.{h,cpp}    vector table, modes, LR offsets, exception entry
  memory.h               IsaEnv: the environment contract a core fills in
  interp.{h,cpp}         the interpreter engine (ARM + Thumb semantics)
  interp_arm.cpp interp_thumb.cpp
  disasm.cpp             the disassembler and the name tables
arch/arm/execution/aarch32/             the AArch32 execution state
  step.{h,cpp}           the pipeline step (prime cur/npc/regs[15], dispatch)
arch/arm/cpu/classic/arm9/arm926ej-s/   the ARM926EJ-S core (this module)
  arm926.{h,cpp}       module registration/factory
  core/                Cpu (inherits IsaState): CP15/MMU/TLB, run loop, env
  jit/                 block cache, IR, translator and host code generators
  tests/               ISA/core/JIT/MMU regression suites and corpus
  tools/               CPU-specific development tools
```

`Cpu` inherits the architectural state (`struct Cpu : public IsaState`) and
forwards the engine entry points (`exec_arm`, `exec_thumb`,
`fetch_and_execute`, ...) into those layers, so the interpreter, the JIT's
out-of-line helpers and the tests all drive one implementation of each rule.
The dependency direction is one-way: the core depends on the ISA and the
execution state, never the reverse.

The JIT is split one step further because `x86_64` and `aarch64` are **host
backends**, not guest ARM architectures:

```text
.../jit/
  backends/
    x86_64/             host x86-64 code generator
    aarch64/             host AArch64 code generator interface
```

This prevents a common error where `arm64` ends up meaning both "guest ARM64" and
"host AArch64 JIT backend".

## SoC boundary

A SoC owns the integration of a CPU core with system-level hardware. Typical
contents are:

```text
soc/<vendor>/<soc>/
  <soc>.h/.cpp
  clocks/
  memory-map/
  interrupt-controller/
  dma/
  gpio/
  uart/
  timer/
  storage/
  ...
```

Reusable IP blocks belong in `devices/` when they are meaningfully independent;
SoC-specific wrappers, quirks, clocking, register aliases, reset values and wiring
belong with the SoC.

## Machine boundary

A machine is a concrete system assembled from a SoC/CPU, RAM/ROM, devices, buses,
and firmware layout. The directory exists so an actual emulated target does not
get confused with the reusable chip model.

Boards are named configurations/presets. They should contain as little behaviour
as possible; behaviour belongs to the CPU, SoC, device or bus module named by the
preset.

## Build rule

The build recursively discovers active implementation files below `arch/`,
`devices/`, `loaders/`, `boards/`, `host/` and `debugger/`. Tests are selected
by living under a `tests/` directory (a component's own `tests/`, or the
cross-component `tests/` tree) and are never linked into the library.

`builtin/builtin.cpp` remains the explicit registration list because this project
uses a static library: an unreferenced module object is otherwise liable to be
omitted by the linker.

## Reference material

Reference manuals are organized by what they describe:

```text
reference/arm/architecture/armv5/
reference/arm/cores/arm9/arm926ej-s/
reference/soc/nxp/imx28/
```

This keeps architecture references next to the concepts they constrain without
making the source tree depend on a particular vendor or board.

## Legacy archive

`old/` is excluded from the active design. It is preserved exactly as supplied and
must not receive source or documentation changes during layout work.
