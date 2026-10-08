# ARM926EJ-S

Concrete ARM926EJ-S CPU core module. Architectural target: ARMv5TEJ, AArch32.

This directory owns the **core**: CP15 (system control coprocessor), the
MMU/TLB, the configuration caches derived from them, the run loop, the JIT,
core tests, and CPU-specific development tools. It does not own the i.MX28
SoC, and it no longer owns the instruction set.

The instruction set is the ARMv5TEJ ISA layer, `arch/arm/isa/armv5/`
(decoders, interpreter engine, PSR/flag model, register banking, exception
model — reusable by any ARMv5TEJ core), and the AArch32 execution state is
`arch/arm/execution/aarch32/` (the pipeline step). `Cpu` inherits the
architectural state (`IsaState`) and forwards the engine entry points into
those layers; see `core/README.md` and `docs/ARCHITECTURE.md`.
