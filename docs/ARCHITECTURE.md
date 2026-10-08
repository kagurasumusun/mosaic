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
      aarch32/          32-bit Arm execution state
      aarch64/          64-bit Arm execution state
    isa/
      armv4/ ... armv9/ architecture generations
    cpu/
      classic/arm7/     classic Arm7-family cores
      classic/arm9/     classic Arm9-family cores
        arm926ej-s/     the current ARM926EJ-S module
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
  console/ timer/ interrupt/ system/ ... reusable peripherals

loaders/
  elf/elf32/ raw/       guest image loaders

host/
  audio/ io/ monitor/    host-side services

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
arch/arm/isa/armv5/
        |
        |  ARM926EJ-S implements ARMv5TEJ (AArch32)
        v
arch/arm/cpu/classic/arm9/arm926ej-s/
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
CPU directory.

## CPU module boundary

The core exposes a small `CpuCore` contract. An individual CPU model implements
that contract and owns everything required to emulate that CPU accurately:
register state, decoding, execution semantics, MMU/CP15 state where applicable,
and CPU-specific translation/JIT code.

For ARM926EJ-S the module is deliberately self-contained:

```text
arch/arm/cpu/classic/arm9/arm926ej-s/
  arm926.{h,cpp}       module registration/factory
  cpu/                 CPU state, decoder and interpreter
  jit/                 block cache, IR, translator and host code generators
  tests/               ISA/core/JIT/MMU regression suites and corpus
  tools/               CPU-specific development tools
```

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
`devices/`, `loaders/` and `boards/`. Tests are selected by living under a
`tests/` directory and are never linked into the library.

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
