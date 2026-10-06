# Architecture

Everything below is what the code actually does today, not what it is planned to
do. Where a decision comes from the processor's documentation it says so, because
the ARM926EJ-S TRM (ARM DDI 0198E, in `reference/`) is the authority this project
follows — not Linux's expectations, not QEMU's defaults, not ARMv7/v8 syntax.

## Layers

```
include/brainemu/brainemu.h    public C API: create/load/run/state/stats
include/brainemu/bus.h         Bus: regions, per-page descriptors, byte access
src/core/state.h               Cpu: the register file, CP15, TLB, inlines
src/core/decode_arm.cpp        ARM instruction -> ArmDec
src/core/decode_thumb.cpp      Thumb halfword -> ArmDec (the same struct)
src/core/interp_arm.cpp        executes an ArmDec: the one semantic engine
src/core/helpers.cpp           decoder helpers + the disassembly printer
src/core/cp15_mmu.cpp          CP15, FCSE, translation, TLB, fault reporting
src/core/bus.cpp               region lookup, byte/sized access, code watch
src/jit/                       block cache, links, invalidation (no backend yet)
src/loader/elf.cpp             ELF32 PT_LOAD placement, raw image placement
src/api.cpp                    the C API over the above
```

`Cpu` keeps the fields generated code will need at fixed, documented offsets
(comments in `state.h`): `regs[16]`, `npc`, `cur`, `insn`, `cpsr`, `vflag`,
`thumb`, `halted`, `exit_req`, `cycles`, `insns`, `jit_flags`, `trans_gen`,
`bus`, `jit`, `ram`, `ram_gbase`, `ram_mask`, `ram_fast`, `exc*`. Keeping them
adjacent and *not* reordered by the compiler is what lets a future backend
address them with a base register plus immediate; a new field therefore goes at
the end of the hot block, not in the middle.

## Two states, one engine

`decode_thumb` fills the same `ArmDec` as `decode_arm`, and
`Cpu::exec_arm_dec` is the only place semantics live. That is a deliberate
reflection of the architecture (ARM ARM A6: "most Thumb instructions are the
ARM instruction with the same name"), and it means the corpus tests the
translation from Thumb encodings to ARM semantics rather than a second
implementation that can drift. Where the two states genuinely differ — `LSLS`
by zero, the block-transfer direction, the `PUSH/POP` register order, the
PC-as-operand rules — the difference is expressed in the decoder, never in a
second interpreter.

## The invariants worth knowing before editing

* `regs[15] == cur + (thumb ? 4 : 8)` — the register file holds the pipeline's
  PC, not the address of the instruction being executed. A write to the PC sets
  `npc` and nothing else; `cur`/`regs[15]` are refreshed on the next fetch.
* `npc` is the address of the *next* instruction, and a link is computed from
  `cur` - never from `regs[15]`, which already points past the instruction. `BL`
  and `BLX` to an immediate in ARM state link `cur + 4`, the latter with bit[0]
  set so that a `BX R14` from the Thumb handler returns to ARM state, and a
  Thumb long branch links
  `(cur + 2) | 1` in the first halfword and folds the second half into it (the
  pair is two independent instructions on this core, so an interrupted sequence
  behaves as on hardware). `tests/data/corpus.txt` locks the branch rules down:
  every `B/BL` case lands on one of twelve slots with a unique effect and moves
  the link into a general register, so both a mis-scaled offset and a wrong link
  change a compared register.
* Only the registers a mode actually banks move on a mode change (DDI 0198E
  4.1.1): R13 and R14 in every mode, plus R8-R12 in FIQ alone. R0-R7 - and
  R8-R12 outside FIQ - name the same physical register in all modes, so
  `flush_bank()`/`load_bank()` must not copy them; a whole-bank copy wipes the
  faulting thread's arguments on any exception. The live view of R8-R12 is kept
  in the user bank so that leaving FIQ restores it.
* `bank[bank_idx] == regs` for R13/R14 and (in FIQ) R8-R12. `Cpu::set_reg`
  therefore writes through to the live bank, which is what lets the ELF loader,
  `brain_set_reg` and a debugger set a mode's stack pointer without the next
  mode change reverting it; tests no longer have to seed both.
* `SPSR_<mode>` is not a mirror of the CPSR: it holds what an exception saved on
  entry, and only `enter_exception()` writes it (plus `MSR` to SPSR). Saving the
  current CPSR in `flush_bank()` looks harmless and silently destroys the value
  a handler is about to return with, whenever an exception or a mode change
  happens inside another handler.
* A faulting instruction fetch still retires the instruction: `step_one()` must
  not validate a fetch of its own, because at that point `cur` is still the
  address that has just executed, so the abort is re-taken for the same address
  for ever and `npc` never reaches the vector. `prepare_fetch()` runs inside
  `fetch_and_execute()`, after `cur = npc`, and the `insns++`/`cycles++` charge
  for the bad fetch belongs to the fetch itself. Without this a guest that turns
  the MMU on before its table maps the vectors - a classic bootloader bug -
  wedges any bounded run instead of faulting 200 times.
* MMU off means **PA = MVA = VA**: no FCSE, no top-byte clearing. i.MX28 puts
  its peripherals at 0x80000000, so boot code must be able to store there before
  a page table exists (`tests/test_unit.cpp` locks this in).
* FCSE (MMU on): `MVA = VA + (FCSE_PID x 32MB)` for `VA < 32MB`, addresses above
  32MB unmodified, PID is 7 bits, and entry-based cache/TLB operations see
  `VA == MVA` (DDI 0198E 2.3 "FCSE PID Register").
* CP15 c1 uses DDI 0198E Table 2-11: `M[0] A[1] C[2] B[7] S[8] R[9] I[12]
  V[13] RR[14] L4[15] X[19]`, with `[6:3]` and `[16],[18]` SBO and `[17],[11:10]`
  SBZ — so the reset value is `0x00050078`, `A = 0` (unaligned ARM data
  processing is legal, with the low bits ignored) and `B` is bit 7, *not* bit 5.
* Any change to a translation-visible state bumps `trans_gen`; every TLB entry
  and every compiled block stores the generation it was built under. A new CP15
  bit that affects addressing must go through `on_translation_env_changed()`,
  otherwise it will be honoured only after the next TLB miss.
* `PG_CODE` + `smc_watch`: pages that have supplied an instruction fetch are
  checked on store, so self-modifying code invalidates the affected blocks
  instead of executing stale ones.

## Bus and devices

`Bus` keeps a two-level page table (1024 × 1024 × 4 KiB = the whole 32-bit
space, allocated lazily) plus a flat list of regions. Mapping is *all-or-nothing*
in both directions: a bulk load that does not fit the RAM window writes nothing,
and `map_device` refuses a span that overlaps anything already present — a
device silently shadowed by the identity fast path would be invisible to the
guest, which is a much worse failure than a refused mapping. One guest access is
one hook call at the natural size (`LDR`/`STR` reach `read`/`write` with `size`
4; the byte-level fallback is used for sizes/conditions the region cannot serve
directly). `tests/test_unit.cpp` asserts that count, because a UART FIFO does not
tolerate being read four times per instruction.

## Adding a JIT backend

`src/jit/jit.h` is the contract: `Jit::run/run_insns`, `invalidate_range`,
`flush_all`, `on_exception`, `on_env_change`. A backend translates an `ArmDec`
chain into a host block that reads and writes the `Cpu` fields listed above and
exits back to the driver on anything it does not handle (helper calls are counted
in `Stats::helper_calls`). The block cache, the links, the code watch and the
`trans_gen` invalidation already exist; only codegen is missing, so the driver
currently delegates to `Cpu::fetch_and_execute`. `build/bench --jit` prints
`interp N jit 0` until that changes — that line is the test that a backend has
been wired in.

## Interworking, the T bit, and branch targets

`Cpu::thumb` is the single source of truth for the current state, and bit 5 of the
stored `cpsr` is kept equal to it by `Cpu::set_thumb()`.  Three readers look at
bit 5 rather than at `Cpu::thumb`: `MRS Rd, CPSR`, the `SPSR_<mode>` written on
exception entry (which is what lets `MOVS PC, R14` return to the state the
exception came from), and `brain_get_reg(BRAIN_REG_CPSR)`.  `MSR` cannot write bit
5 - the mode/flag write masks it out - so the mirror is only ever updated by an
instruction that is architecturally allowed to change the state: `BX`/`BLX`, the
Thumb long-branch pair, `BLX` with an immediate, and a load into R15.

Branch target masking follows the state, not the encoding: a Thumb target is masked
to 2 bytes, an ARM target to 4.  For `BLX` with an immediate the low half of the
offset *is* bit 1 of the target, so masking that to a word boundary discards a real
address bit; masking a Thumb branch target to a word boundary drags every target
that is 2 mod 4 back onto the preceding halfword, which is how a `b .` loop came to
run two bytes early.

`Cpu::load_pc()` honours SCTLR.L4 (c1[15], reset 0): with L4 = 0 a load into the PC
takes the state from bit 0 of the loaded value and clears that bit, with L4 = 1 it
leaves the state alone and drops bits[1:0] instead.  Non-load writes to the PC in
ARM state (including `MOV PC, Rn` and `LDM` without the S bit) ignore bits[1:0];
only an SPSR-restoring return takes the state from the saved `CPSR.T`.
