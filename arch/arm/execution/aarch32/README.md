# AArch32 — 32-bit Arm execution state

32-bit Arm execution state using the A32/T32 instruction encodings. It covers
multiple architectural generations and must not be confused with the ABI name
`armeabi-v7a`.

This directory is the **execution-state** layer, between the ISA and the core:

* below it, the ISA (`arch/arm/isa/armvN/`) — what an instruction *does*
  (decoders, execution semantics, PSR, exceptions), written against
  `IsaState` + `IsaEnv`;
* above it, the core (`arch/arm/cpu/...`) — run control (budgets, halting,
  stop requests, the JIT dispatcher) and the environment itself (memory and
  translation, the coprocessor, exception entry, JIT notification).

What is AArch32 here, and not ISA: the two instruction states (ARM and Thumb,
selected by the T bit), and the pipeline rule `regs[15] == cur + 8` (ARM) /
`cur + 4` (Thumb) that every instruction reads the PC through.

| file | what it holds |
| --- | --- |
| `step.h`, `step.cpp` | `fetch_and_execute()` — the pipeline step: prime `cur`/`npc`/`regs[15]`, translate the fetch through the environment, dispatch to the state's engine, take a prefetch abort immediately. |

An AArch64 execution state (`arch/arm/execution/aarch64/`) would be the same
shape over a different ISA and register file — one step driver, one state
enum, no ARM/Thumb interworking.
