# brain-emu

`brain-emu` is a modular system emulator. The repository is organized by the
thing being modeled, not by one historical machine.

## Layout

```text
core/                                  emulator foundation and public API
arch/                                  guest architectures and CPU cores
  arm/
    execution/aarch32|aarch64/         execution state / ISA width
    isa/armv4..armv9/                  architectural generations
    cpu/classic|cortex-a|r|m|neoverse/ concrete CPU cores
    .../jit/                           CPU translation and host backends
soc/                                   SoC-level integration and registers
machine/                               concrete machine/system compositions
boards/                                reusable named board presets
buses/                                  reusable interconnects
 devices/                               reusable memory-mapped peripherals
loaders/                                guest image formats
host/                                   host-side services: log, byte I/O, filesystems,
                                        monitors (log/ io/ fs/ monitor/)
debugger/                               debugger/trace integrations (gdb/)
testkit/                                shared test helpers
tests/                                  cross-component tests
tools/                                  command-line utilities
builtin/                                explicit built-in module registry
docs/                                   design and coverage documentation
reference/                              architectural and vendor manuals
old/                                    untouched legacy archive
```

## ARM naming rule

Do not use `arm32/` and `arm64/` as the primary architecture hierarchy.

* `AArch32` and `AArch64` are execution states and live under
  `arch/arm/execution/`.
* `Armv4` through `Armv9` are architecture generations and live under
  `arch/arm/isa/`.
* `ARM926EJ-S`, `Cortex-A53`, etc. are concrete CPU cores and live under
  `arch/arm/cpu/`.
* An SoC such as `i.MX28` lives under `soc/`, because it is a chip integrating
  a CPU core with peripherals and system logic.
* A real board/system built around that SoC lives under `machine/`.
* A named reusable configuration/preset lives under `boards/`.

For example:

```text
ARM926EJ-S
  ISA: Armv5TEJ
  execution state: AArch32
      |
      v
soc/nxp/imx28/
      |
      v
machine/arm/<concrete-system>/
```

This keeps ISA, CPU implementation, SoC and machine composition independent, so
adding another ARM9 core or another AArch64 SoC does not force a directory-level
rewrite.

## Current implementation

The first real CPU core is the ARM926EJ-S, and it is layered the way the
naming rule describes:

* `arch/arm/isa/armv5/` is the ARMv5TEJ **instruction set architecture** —
  the ARM and Thumb decoders, the execution semantics (the interpreter
  engine), the barrel shifter, the PSR/flag/mode model, register banking and
  the exception model. It is a real, separable layer: it knows nothing about
  any core, and runs against an architectural state (`IsaState`) plus an
  environment contract (`IsaEnv`) a core fills in.
* `arch/arm/execution/aarch32/` is the **AArch32 execution state** — the
  pipeline step that primes `cur`/`npc`/`regs[15]` and dispatches to the
  state the T bit selects.
* `arch/arm/cpu/classic/arm9/arm926ej-s/` is the **core** — CP15, the
  MMU/TLB, the cache/alignment/endianness configuration, the run loop, the
  JIT, and the `IsaEnv` the engine calls through. `Cpu` inherits `IsaState`
  and forwards the engine entry points, so the interpreter, the JIT and the
  tests all drive one implementation.

The ARM926EJ-S's MMU/CP15 model, JIT, tests and tools live under
`arch/arm/cpu/classic/arm9/arm926ej-s/`.

The synthetic `toy8` CPU remains available as a module-seam test, but is now
clearly separated under `arch/experimental/toy8/` rather than being presented as
a real processor family.

The current SoC/documentation slot for the NXP i.MX28 is
`soc/nxp/imx28/` and `reference/soc/nxp/imx28/`. The implementation can be added
without moving the ARM926 CPU again.

## The host stack

The emulator's common foundation for everything around the guest lives in
`host/` (logging, byte I/O, filesystems, monitors) and `debugger/` (the gdb
stub), with the UART — the peripheral a real firmware talks to — in
`devices/uart/`. All of it works through the C API (`brain_emu_t*`), so none of
it knows a CPU:

* `host/log/` — one leveled logger for the whole tree.
* `host/io/` — byte sinks/sources (stdio, buffers) and the adapters that turn
  them into the function pointers a device takes.
* `host/fs/` — the filesystem service: `HostFs` (rooted at a host directory, a
  sandbox against `..`) and `MemFs` (in memory).
* `host/monitor/` — a `stats` monitor module (`--monitor stats`) and the command
  monitor, a debugger's console (`--monitor-cmd`).
* `debugger/gdb/` — a gdb stub: the remote serial protocol over TCP
  (`--gdb PORT`) or over a memory transport in tests.
* `devices/uart/` — a 16550-style UART: 8 byte-wide registers, a 16-byte RX
  FIFO, one interrupt output, host hooks for where the bytes go and come from.

`tests/integration/test_host_stack.cpp` exercises the whole stack together: a
guest printing through a console and a UART, an interrupt over a wire into the
CPU, and the gdb stub and command monitor driving a machine through the C API.

## Build

```sh
make
make check
make modules
```

CMake is also supported. Source discovery is recursive so subsystem depth is not
part of the build contract.

## `old/`

`old/` is an archive and is deliberately excluded from the active layout. It is
not edited, renamed, or normalized by this reorganization.
