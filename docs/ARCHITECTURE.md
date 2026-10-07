# Architecture

Everything below is what the code actually does today, not what it is planned to
do. Where a decision comes from the processor's documentation it says so, because
the ARM926EJ-S TRM (ARM DDI 0198E, in `reference/`) is the authority this project
follows — not Linux's expectations, not QEMU's defaults, not ARMv7/v8 syntax.

## Layers

The emulator is one core plus modules: the core owns the address space, the run
loop and the C API, and a module owns one piece of the emulated world.  The
ARM926EJ-S is the first CPU module, not the shape of the emulator
(`docs/MODULES.md` is the guide to the split, and to adding a processor).

```
include/brainemu/brainemu.h        public C API: create/load/run/state/stats
include/brainemu/core/bus.h        Bus: regions, per-page descriptors, byte access
include/brainemu/core/module.h     ModuleInfo, CpuModule, ModuleRegistry
include/brainemu/core/machine.h    MachineConfig, Machine (bus + CPU + devices)
include/brainemu/cpu/cpu.h         CpuCore: the whole CPU contract
include/brainemu/device/device.h   Device, FuncDevice, BufferDevice, ConsoleDevice
include/brainemu/loader/loader.h   Loader (probe + load) + ImageInfo
src/core/bus.cpp                   the address space (RAM, ROM, devices, watch)
src/core/device.cpp                the device base class and its adapters
src/core/module.cpp                the registry
src/core/machine.cpp               build, run, reset, load
src/loader/elf.cpp                 the elf32 + raw loader modules
src/modules/builtin.cpp            the list of modules this build contains
src/api.cpp                        the C API over the Machine

src/modules/arm926/arm926.cpp      the ARM926 module: its CPU module + adapter
src/modules/arm926/state.h         Cpu: the register file, CP15, TLB, inlines
src/modules/arm926/decode_arm.cpp  ARM instruction -> ArmDec
src/modules/arm926/decode_thumb.cpp  Thumb halfword -> ArmDec (the same struct)
src/modules/arm926/interp_arm.cpp  executes an ArmDec: the one semantic engine
src/modules/arm926/helpers.cpp     decoder helpers + the disassembly printer
src/modules/arm926/cp15_mmu.cpp    CP15, FCSE, translation, TLB, fault reporting
src/modules/arm926/jit/            block cache, IR, x86-64 + AArch64 backends
src/modules/toy8/                  a second CPU module, to keep the core honest
```

The core talks to a processor only through `CpuCore`, and only at *slice*
boundaries - run a budget, step once, read a register, reset.  Nothing in
`src/core` knows an opcode, a CPSR or a Thumb bit; nothing in
`src/modules/arm926` knows what a device is.  Two seams are worth naming here
because they used to be the other way round:

* `Bus::set_store_observer(fn, user)` is how the ARM module learns that a store
  landed in a page holding translated code.  It replaced a `Bus::cpu`
  back-pointer, so the address space no longer knows that a processor exists
  (and `PageDesc` stays trivially copyable, which the MMU code relies on).
* `Machine::load_image` places an image and then asks the CPU module to enter it
  (`CpuCore::set_entry`).  Seeding the stack is the module's job because only
  the module knows what a stack pointer is; the module's `CpuInfo` is what tells
  the C API which register index the PC, the flags and the stack pointer are.

`Cpu` (in the ARM926 module) keeps the fields generated code will need at
fixed, documented offsets
(comments in `state.h`): `regs[16]`, `npc`, `cur`, `insn`, `cpsr`, `vflag`,
`thumb`, `halted`, `exit_req`, `cycles`, `insns`, `jit_flags`, `trans_gen`,
`bus`, `jit`, `ram`, `ram_gbase`, `ram_mask`, `ram_fast`, `exc*`. Keeping them
adjacent and *not* reordered by the compiler is what lets a backend address them
with a base register plus immediate (the x86-64 one does, for the helpers it
will call); a new field therefore goes at the end of the hot block, not in the
middle.

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

## The run loop, devices and monitors

`Machine::run_insns`/`run_cycles`/`step` are the only places a CPU module is
asked to execute, and each of them ends the slice in one place: the devices are
ticked with the cycles the slice charged, and the monitors
(`brainemu/core/monitor.h`) are told how many instructions retired.  A monitor
is how a tracer, a profiler or a watchpoint layer observes the machine without
the core growing a feature for it; a hook may call `Machine::stop()`, which is a
stop at the next slice boundary like any other.

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

## The JIT: one interpreter, a translation layer, two backends

`src/modules/arm926/jit/jit.h` is that module's own JIT contract:
`Jit::run/run_insns`, `invalidate_range`, `flush_all`, `on_exception`,
`on_env_change` - internal to the module, because a CPU module is free to have
an execution engine of its own or none at all (toy8 has none). Between the guest
and a backend sit three things that are backend-independent:

* the *block cache* (`cache.cpp`) — one entry per guest basic block, hashed on
  the pc, with `trans_gen` versioning (a control-register write invalidates every
  block, which is what makes a stale block unable to retire one instruction too
  many), a self-modification watch on the code pages, and a negative cache: a pc
  whose translation was refused is not re-attempted until the generation changes
  (a loop ending in a branch is refused once, not once per iteration);
* the *IR* (`ir.h`) — `ArmDec` in, a small flat op list out, with the ARM flag
  rules (shifter carry, V preservation, `MOVS #imm`) resolved once;
* the *front end* (`frontend.cpp`) — decides how far a block may run, fetching
  each word as it goes (identity-mapped RAM straight out of the array, otherwise
  the TLB exactly as `prepare_fetch` does), and gives up *before* the first
  instruction it cannot translate.  What a block may contain is data processing,
  the LDR/STR families, and one terminating branch; what it deliberately leaves to
  the interpreter is listed in `docs/ISA-COVERAGE.md`, and the short version is
  "anything predicated, anything whose flags the host cannot reproduce, and any
  access whose verdict depends on the machine it was translated for".

A block owns its accesses, so it needs a way to change its mind.  `JitBlock::meta`
and the shadow buffer carry the entry handoff (the EFLAGS image, the RAM window, the
code-page map, the fast-memory verdict); the *exit* is a three-way protocol that
`cache.cpp` and the emitters share: a block that ran to its end, a block that
*bailed* at a guarded access (it writes `SH_ABORT_PC`, the count and cost it reached,
and the address to resume at - the interpreter then performs that one instruction,
which is what keeps the SMC and abort paths in one place), and a branch, whose taken
or not-taken outcome feeds `stats.branch_taken`/`branch_not`.  Bit 0 of the address a
block leaves in `SH_R15` is the instruction-set bit for a `bx`/`blx`, so the
dispatcher switches state on the way out - a translated block may *enter Thumb*, it
just never runs a Thumb instruction itself.

A backend only ever sees the IR. Two exist, and the host picks one
(`BRAIN_JIT_BACKEND` in `jit.h`): AArch64 (`emit_arm64.h`), and x86-64
(`emit_x64.cpp`) which is what runs on the machine this was developed on. The
x86-64 backend keeps N, Z and C in the host flag register and V in a register
(`SH_VFLAG`), because x86 has no cheap writer for OF; the dispatcher therefore
hands it an EFLAGS image in the shadow buffer (`SH_EFLAGS`, loadable with a
single `popfq`) and merges the image back into `Cpu::cpsr` afterwards
(`JitBlock::meta` says whether a block touches either half). Every block's exit
writes R15 = the next pc, so the dispatcher's `cur`/`npc`/pipeline view is
reconstructed in one place for both engines.

Correctness is checked by running both engines on identical machines and
comparing: `test_instr --jit` replays the whole ISA corpus through the cache, and
`tests/test_jit_x64.cpp` executes emitted blocks in-process against the
interpreter (every data-processing opcode over a spread of operands, entry flag
sets and rotated immediates; every load/store form over the addresses that matter;
all fifteen branch conditions against all sixteen flag words; the two dispatcher
rules that gate translated accesses; thousands of random blocks and a
register-pressure block) — plus a shadow-buffer sentinel test, because the
offsets the generated code hard-codes cannot be checked by the compiler.

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
to 2 bytes, an ARM target to 4.  `BLX` with an immediate takes its offset from the
A32 A2 layout, `imm32 = SignExtend(imm24 : H : 0)` over insn[23:0] with H =
insn[24]: the offset is an *even* number rather than a multiple of 4, so insn[0] is
part of the displacement (it contributes 4, not a fixed 1) and every `cond = 1111`
word in that opcode space is a BLX, which is what the model this project checks
against does too.  That is why the target is masked to 2 bytes, and why reading the
immediate from insn[23:1] halves every displacement.  For `BLX` with an immediate the low half of the
offset *is* bit 1 of the target, so masking that to a word boundary discards a real
address bit; masking a Thumb branch target to a word boundary drags every target
that is 2 mod 4 back onto the preceding halfword, which is how a `b .` loop came to
run two bytes early.

`Cpu::load_pc()` honours SCTLR.L4 (c1[15], reset 0): with L4 = 0 a load into the PC
takes the state from bit 0 of the loaded value and clears that bit, with L4 = 1 it
leaves the state alone and drops bits[1:0] instead.  Non-load writes to the PC in
ARM state (including `MOV PC, Rn` and `LDM` without the S bit) ignore bits[1:0];
only an SPSR-restoring return takes the state from the saved `CPSR.T`.
