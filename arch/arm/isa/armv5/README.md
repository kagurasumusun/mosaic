# `isa/armv5/` — the ARMv5TEJ instruction set architecture

This directory is the **ARMv5TEJ ISA layer**: everything the architecture
defines, and nothing that belongs to a particular core. It is the reusable
unit of the emulator — any ARMv5TEJ core drives it, and it has no dependency
on any core.

| file | what it holds |
| --- | --- |
| `state.h`, `state.cpp` | `IsaState` — the architectural state (register file, pipeline view, CPSR + V mirror, Thumb bit, privilege mirror, bank array, counters, pending-exception state) and the architectural rules that operate on it: condition evaluation, flag helpers, register banking, PC writes. Plus the ISA's enums: modes, banks, CPSR bits/masks, condition codes, the exception vectors. |
| `ops.h` | The datapath primitives both engines share: the barrel shifter, ARM's carry/overflow arithmetic, the ARMv5E saturating helpers, the data-processing opcode numbering, and the cycle model (`CycleModel`) the interpreter and the JIT charge identically. |
| `decode.h`, `decode_arm.cpp` | The A32 (ARM state) decoder. |
| `decode_thumb.h`, `decode_thumb.cpp` | The T32 (Thumb state) decoder, mapping every 16-bit format onto the same `ArmDec`. |
| `exceptions.h`, `exceptions.cpp` | The exception model: the vector table, exception modes, LR offsets, the ARMv5 fault-status encoding, the pending-exception raisers, and the *architectural* half of exception entry (`enter_exception`: banking, SPSR, LR, CPSR mode/I/F/E/T, PC := vector). |
| `memory.h` | `IsaEnv` — the environment contract: the function-pointer table the embedding core fills in (translated memory read/write/fetch, `prepare_fetch`, coprocessor present/write/read for MCR/MRC, `take_exception`, `notify_jit`, `sync_caches`, and the stats sink). Also the memory-access flags (`MF_*`). |
| `interp.h`, `interp_arm.cpp`, `interp_thumb.cpp` | The interpreter engine: the execution semantics of every ARMv5TEJ instruction as free functions over `IsaState&` + `IsaEnv&`, plus the operand/addressing/PSR helpers both instruction states share. |
| `disasm.cpp` | The disassembler (`format_arm_insn`/`format_thumb_insn`, driven by the real decoders) and the name tables (`cond_name`/`mode_name`/`exc_name`). |

## What this layer deliberately does not know

- **Memory and translation.** Loads/stores/fetches go through `IsaEnv`. A core
  with no MMU, a core with a different MMU, or a unit test with a flat array
  all supply their own answers.
- **Coprocessors / system control.** MCR/MRC are answered through
  `IsaEnv::coproc`. CP15 — the ARM926EJ-S's system control coprocessor, its
  TLB, its cache/TCM configuration — is a core
  (`arch/arm/cpu/classic/arm9/arm926ej-s/`).
- **Run control.** Instruction budgets, halting, stop requests, JIT
  notification policy and statistics accounting beyond the counters the engine
  itself keeps are the embedding core's policy.

Three fields at the end of `IsaState` are *core-maintained behaviour policy*
the engine reads but never derives: `l4` (SCTLR.L4 — "loads to PC also load
the T bit", which on this architecture generation comes from system control,
not from the ISA), `jazelle_trap` and `strict_unpredictable` (emulator
strictness options). The embedding core keeps them current.

## How a core embeds it

By inheritance: `struct Cpu : public IsaState` (the ARM926EJ-S does exactly
that), so the engine, the core's own code, the JIT helpers and the tests all
operate on one object with one register file. The execution-state driver one
level up (`arch/arm/execution/aarch32/step.cpp`) steps it.

## Conformance

Pinned by the ARM926EJ-S core's suites
(`arch/arm/cpu/classic/arm9/arm926ej-s/tests/`): the instruction corpus
(577 interpreter + 577 JIT cases cross-checked against an independent oracle),
the instruction-level unit tests (1054 checks), the MMU tests (361), and the
x86-64 JIT tests (127). The decoder is additionally cross-checked against
capstone by `tests/tools/check_decoder.py` when capstone is installed.

References: ARM DDI 0100I (ARM ARM v5) — `reference/arm/architecture/armv5/DDI01001.pdf`;
ARM DDI 0198E (ARM926EJ-S TRM) — `reference/arm/cores/arm9/arm926ej-s/`.
