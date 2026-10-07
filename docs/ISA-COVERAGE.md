# ISA coverage

Status of the ARMv5TEJ / ARM926EJ-S instruction set as implemented by
`src/modules/arm926/decode_arm.cpp`, `src/modules/arm926/decode_thumb.cpp` and the single execution
engine in `src/modules/arm926/interp_arm.cpp`. "Checked" below means a case in
`tests/data/corpus.txt` compares this core against a golden emulator for that
encoding; 350 cases exist today (248 in ARM state, 102 in Thumb: single
encodings, short sequences that carry a flag or a register from one instruction
into the next, 59 randomised multi-instruction sequences, and three
self-modifying programs). The same corpus is replayed a second time through the
JIT when a backend is compiled in (`test_instr --jit`), so every "checked" below
is checked on both execution paths.

The design rule for anything not on this list: an unimplemented *encoding* must
decode to `AOP_UNDEF` and take the undefined-instruction exception. Nothing may
be silently ignored — a decoder that accepts a word it does not implement is
worse than one that faults, because the guest then sees a plausible-looking
machine.

## ARM state

| Group | Encodings | Status |
| --- | --- | --- |
| Data processing | `AND EOR SUB RSB ADD ADC SBC RSC TST TEQ CMP CMN ORR MOV BIC MVN`, immediate and register forms, all four shifts plus RRX, register-shift | implemented, checked |
| Multiply | `MUL MLA` (with and without flags), `UMULL UMLAL SMULL SMLAL`, and the ARMv5E DSP forms `SMULxy`, `SMLAxy`, `SMLALxy` | implemented (`AOP_MUL`, `AOP_MULL`, `AOP_MULH`, `AOP_MULHAL`); `MUL/MLA` and the `xy` forms are checked |
| Saturating add / subtract | `QADD QSUB QDADD QDSUB` (the four `kind` values in the multiply/SATISFY space) | implemented (`AOP_QD`); `QADD` and `QSUB` are checked, including the Q flag |
| Count leading zeros | `CLZ` | implemented, checked |
| Halfword / signed loads | `LDRH LDRSH LDRSB STRH`, immediate-offset, register-offset, pre/post index with writeback, and the unaligned-permitted forms | implemented (`AOP_LDRSTH`), checked for both offset kinds |
| Dual and single swaps | `LDRD STRD`, `SWP SWPB` | implemented, checked |
| Load/store | `LDR STR LDRB STRB`, immediate / register offset, pre/post, writeback, negative offsets, PC-relative reads | implemented (`AOP_LDRST`), checked; the store data register is `Rd[15:12]`, which is a classic decoder trap |
| Block transfer | all eight `LDM/STM` addressing modes, user-register list, PC in list, writeback | implemented, checked. An empty register list is UNPREDICTABLE in ARMv5: the ARM family retires the base update and moves no registers (what the hardware does), while the Thumb `PUSH/POP/LDM/STM` forms decode as undefined |
| Status | `MRS`, `MSR` (c/f/s/x masks, SPSR in privileged modes, undefined in USR) | implemented, checked |
| Branch | `B BL`, `BX`, `BLX` (immediate and register), condition codes | implemented, checked (`BL` links `cur+4`, Thumb-entering `BLX` sets T) |
| Coprocessor | `MRC MCR` for p15, `LDC/STC` (any coprocessor), `CDP` | p15 `MRC/MCR` implemented and checked (decode *must not* turn them into undefined); `CDP` and `LDC`/`STC` for *every* number - p15's interface is MRC/MCR only - decode as UNDEF, matching a core with no coprocessor, pinned by the `LDC`/`STC` rows of `test_exception_entry()` |
| Software interrupt / debug | `SWI`, `BKPT` | implemented, checked |
| Hardware interrupts | `nIRQ`/`nFIQ` level inputs driving entry (0x18/0x1C vectors, I/F masking, SPSR, link adjustment, forced ARM state) | implemented (`brain_set_irq`/`brain_set_fiq`), checked by `test_interrupt_inputs()` in both states, including entry from Thumb reached through a taken `BX` |

## Thumb state

| Group | Encodings | Status |
| --- | --- | --- |
| Shift by immediate | `LSL LSR ASR` (a shift of 0 is the ARMv5 `MOV`, which does *not* update flags) | implemented, checked |
| Add/subtract, compare | `ADD SUB ADDS SUBS CMP CMN` register and 3-bit immediate | implemented, checked |
| ALU, one and two registers | `AND EOR LSL LSR ASR ADC SBC ROR TST NEG CMP CMN ORR MUL BIC BICS MVN ANDS EORS ADCS SBCS RSBS MULS LSL...S` | implemented, checked |
| Special data processing (high registers) | `MOV ADD CMP` with R8-R15, `BX BLX`, and the `NOP`-ish encodings | implemented, checked (`MOV SP, Rn` included; `H` bit handling is `Rdn = bit7 | Rd`) |
| Loads and stores | PC-relative, SP-relative, register offset (all 8 forms incl. `LDRH LDSH LDSB STRH`), immediate offset halfword | implemented, checked with the base register inside the compared memory window |
| Miscellaneous | `NOP`-class hints, `ADD Rd, PC/SP`, `ADDS/SUBS` with 8-bit immediate, `ADD SP, #imm`, `MOV R8-R15` | implemented, checked |
| Stack operations | `PUSH POP` (with and without `LR`), `LDMIA STMIA` | implemented, checked. `POP` never restores CPSR — only the T bit follows the branch, as on hardware |
| Conditional branch, SWI | `BEQ..BVC`, `SWI` | implemented, checked: all 14 conditions, each with a target that is word aligned and one that is only halfword aligned, so a wrongly scaled offset or a target masked to 4 bytes shows up as a different R0 |
| Block transfer direction | `STMIA LDMIA` are both "increment after", `P = 0, U = 1, W = 1` | checked |

## Self-modifying code

ARMv5 makes self-modifying code UNPREDICTABLE without the cache maintenance it
would need on hardware (a `DCache` clean and an `ICache` invalidate around the
store).  A model has to pick something, and this core picks the boring,
deterministic reading: **an instruction word is read from memory for every
retired instruction**, so a store is visible to the next fetch, and both
execution paths have to behave as if they fetched every word.  The interpreter
does that by construction.  The JIT has to be *told*, because a translated block
is host code with the guest words already compiled in:

* installing a block flags every 4 KiB guest page it was built from
  (`Bus::mark_code_page`, the `PG_CODE` bit the bus has had since the first
  round);
* `Cpu::smc_watch` is set while any page carries that flag, and the store path
  in both the fast RAM case and the MMU slow path then asks `Bus::note_store`,
  which reports a store to a flagged page to the CPU and drops the blocks that
  overlap it (`Cpu::invalidate_code` → `Jit::invalidate_range` →
  `jit_cache_invalidate`, which unlinks only the blocks whose `[pc, end_pc)`
  really intersects the store);
* a flush - the one operation that drops every block - clears the flags and the
  guard with them, so the interpreter alone never pays for any of it.

The granularity is per page and deliberately conservative: a flagged page with
no block left costs one range check per store, while a live block can never sit
on a page whose flag has been cleared, which is the property that matters.
`tests/test_unit.cpp::test_self_modifying_code()` pins the plumbing directly
(the observer is called for a store to the code page and not for one to a data
page, a covering store drops the block, a flush clears flag and guard) and
`Stats::smc_flushes` counts the drops.

Three corpus cases pin the behaviour against the oracle, all with the patch
landing on a word the store has already passed: `arm_smc_patch_loop` (the loop
head becomes a different immediate - a build that keeps running the stale block
ends on 400 instead of 599, which is measured, not argued),
`arm_smc_patch_same` (the same store writing the word that is already there, the
control) and `arm_smc_countdown` (the patch turns the loop head into
`SUBS R0, R0, #1`, so the program ends by itself and a stale `ADD` block shows
up both in R0 and in CPSR).  A patch that lands on a word *ahead* of the store
in the same straight-line run is deliberately absent: QEMU keeps executing its
stale translation of that run, so the oracle's answer depends on its
translation-block granularity rather than on the architecture.  The three cases
here are the ones where an implementation that refetches every word and one that
caches translations agree.

## Deliberately undefined

* 32-bit Thumb encodings: `IT`, `CBZ/CBNZ`, `MOVW/MOVT`, `REV/REV16/REVSH`,
  `SXTH/UXTB/UXTH`, `SMUL*`, `LDRD/STRD` in Thumb, `B.W`, `BL.W`. These are
  ARMv6T2/v7 features; an ARM926EJ-S takes an undefined instruction.
* `MCR2/MRC2` and `MCRR/MRRC` (ARMv7 / ARMv5-EJ extensions this core does not
  have). Note that keystone's "MCR2" word differs from `MCR` only in the
  condition nibble, so the decoder keys `MCR2` on `cond == 0b1111` and not on a
  group bit.
* `SWP`/`SWPB` with a writeback, or with R15 as either data or address register.
  `LDR`/`STR` with `Rn == R15` is *not* refused: it uses the pipeline PC with
  bits[1:0] cleared, for both the ARM family and the Thumb forms.
* `CDP` (no coprocessor to send it to), and `LDC/STC` for any `cp != 15`.
* `DMB DSB ISB WFI WFE SEV YIELD WFE` and the other ARMv6/v7 hints, plus
  `MRS/MSR` of the banked SP_*SR registers (ARMv6+).
* In Thumb, the three holes that surround the stack instructions:
  `1011 001x xxxx xxxx` and `1011 011x xxxx xxxx` (ARMv6T2 `LDM`/`STM` without
  writeback, around `ADD/SUB SP` and `PUSH`) and `1011 1111 xxxx xxxx` (`IT` and
  friends, around `POP`).  Those four groups differ only in bit 9, so keying the
  `PUSH`/`POP` decode on bit 10 alone made the holes execute as a stack op - and
  made the `BKPT` test below it unreachable, since `1011 1110` was already
  claimed.  All three holes now take an undefined instruction; `BKPT` reaches its
  own operation again.  `tests/test_unit.cpp::test_exception_entry()` uses `IT AL`
  (`0xBFE8`) as its Thumb undefined trigger, so the group stays honest.
* **Jazelle (T16)** is not implemented: no bytecodes are decoded. With
  `cfg.jazelle_trap` set (the default) an `MSR` that would set `CPSR.J`, and a
  `BX` whose target has bit[1] set from ARM state, take an undefined instruction;
  with the trap cleared, `CPSR.J` becomes observable but the next fetch is still
  ARM or Thumb per `CPSR.T`. A real ARM926EJ-S runs the bytecode interpreter, so
  this is a gap rather than a detail.

## Known divergences that are not chased

* **Thumb `LSLS/LSRS/ASRS Rd, Rm, #0`** — ARMv5 treats these as a `MOV` that
  leaves the flags alone, ARMv7 sets N and Z. The core follows ARMv5; the golden
  emulator follows a v7 profile, so the case is excluded from the corpus instead
  of being "fixed" to match.
* **CP15 reset values.** The golden machine's CP15 c1 comes from QEMU's own
  default, which the QEMU developers have documented as not matching the TRM
  (bits 16/18 are SBO and 17 is SBZ on this core). `MRC` of c1 is therefore
  *decoded and executed* by the corpus but its returned value is not compared.
* **Deferred SP/LR writeback.** QEMU retires R13/R14 updates at the end of a
  translation block, so a one-instruction oracle case that reads back a value the
  instruction just wrote to SP or LR cannot be trusted. The corpus covers the
  link registers by following `BL/BLX` with `MOV Rn, R14`-style reads, and
  `run_case` does not compare R13/R14/R15.

## Gaps to close next

1. ~~A code generator for *this* host.~~  Closed: `src/modules/arm926/jit/emit_x64.cpp` is the
   x86-64 backend, selected by `BRAIN_JIT_BACKEND` on an x86-64 host, so
   `Stats::insns_jit` is non-zero and the corpus replays through generated
   blocks natively; `test_instr --jit` *fails* if the backend retires nothing,
   and `make check` runs the same corpus both ways.  What is left of this item
   is *coverage* rather than existence: the accepted set below is data
   processing only, so a loop with a load or a branch still runs partly in the
   interpreter (which is what `bench --jit`'s engine split reports).
2. Unaligned device accesses.  (`LDC/STC` for p15 is closed and was based on a
   wrong reading: neither the TRM nor the Microchip SAM9G15 datasheet gives CP15
   a load/store interface - "other instructions like CDP, LDC, STC can cause an
   undefined instruction exception" - which the core already does and now pins.
   `PG_DEVICE` splitting is pinned by `tests/test_mmu.cpp`: aligned word accesses
   are one bus transaction, misaligned ones ascend as single bytes, and `c1.A`
   turns the misaligned case into a pre-bus alignment fault with FAR/DFSR.)
3. Interrupt *timing*, not delivery.  This item is otherwise closed: the public
   API now carries `brain_set_irq()`/`brain_set_fiq()`, and
   `test_interrupt_inputs()` pins entry in both states - the 0x18/0x1C vectors,
   the F/I masks holding a pending line, SPSR carrying the whole pre-entry CPSR
   (T included, when the entry arrives from Thumb through a taken `BX`), and the
   8/6 link adjustment.  What remains unmodelled is latency: the core samples the
   lines at instruction boundaries only, so there is no notion of "interrupt
   arrives mid-block" and cycle-level response time is not defined.
4. Thumb long-branch *pairing*.  The core decodes halfwords independently, so an
   unpaired BL/BLX second half (`0xE800-0xEFFF`, `0xF800-0xFFFF` at an odd fetch
   position) executes as the branch component instead of taking the undefined
   instruction an ARM926EJ-S would on unallocated first-halfword space; the
   related question of what classic Thumb's `0xCC00-0xCDFF` slice means on this
   core (ARMv6 writeback LDM/STM vs any surviving coprocessor form) is likewise
   not pinned.  This is an open question, not a proven defect: classic v5
   leaves standalone second halves UNPREDICTABLE (not undefined), so the model
   may legitimately resolve them to the A7.1.17 branch component as it does.
   Resolution needs DDI0100C A5.2's space tables; then either an orphan latch
   in `Cpu::fetch_and_execute()`, or pin today's choice in `test_instr`.
   Probe: `build/jit_emit --hex 4680,ed94,e7fe,0000 --thumb --exec --steps 4`.
5. Nothing remains of the prefetch side: the `L4` half was closed earlier
   (`Cpu::load_pc()` honours SCTLR.L4 for `LDR`/`LDM`/`POP` into R15 as well as for
   the `MOV`-style PC writes, c1[15] is writable and reads back, and
   `tests/test_unit.cpp` pins both settings), and device-page fetches are closed by
   the model's shape: there is no prefetch pipeline, so a fetch happens exactly once
   per retired instruction and cannot speculative-read a device, while DDI0198's
   "alignment faults are not possible for instruction fetches" holds by
   construction.  What stays unmodelled is an *external* abort on an instruction
   fetch - the data path has `serror_on_busfault`, the fetch path has no analogue.

Two further notes on the interrupt path: exception *return* is exercised through
`MOVS PC,R14` (tests/test_unit.cpp restores CPSR from SPSR and resumes with it);
the other return idioms are pinned only through the link values they consume -
`test_interrupt_inputs()` checks that `SUBS PC,R14,#4` would resume exactly on the
deferred instruction, but does not run the pair back-to-back with a handler
return.  A dropped line must be re-asserted for a second entry - `test_interrupt_inputs()`
parks the handler's spin on the low edge - because there is no acknowledgement
input: the semantics are the pins', not a real interrupt controller's.

## How a case can end up not covered at all

`gen_instr_tests.py` assembles each line with Keystone and *rejects* any line whose
word is not an ARMv5 16-bit Thumb / classic ARM encoding (a `MOVS R1, #1` written
where Thumb-16 needs it, say, and Keystone answers with a 32-bit `mov.w`).  A
rejected line drops the whole case, and the run reports it under "skipped".  That
is the right failure mode - the corpus never contains an encoding this core does not
implement - but it does mean a table row can claim coverage the file does not have:
the Thumb conditional branches were listed as checked while every one of them was
being skipped, which is how a Thumb `B<cond>` reading its condition field from the
wrong five bits survived.  When adding a row here, confirm the case count moved
(`wrote N cases`) rather than trusting the generator to have emitted it.

The generator now enforces the three habits below, so they do not have to be
remembered:

* every dropped case is reported under the reason it was dropped - `encoder` (the
  line would not assemble, or assembled to a width ARMv5 does not have), `oracle`
  (Unicorn refused to run it) or `policy` (it is deliberately left out) - as in
  `skipped 55 (encoder 41, oracle 8, policy 6)`;
* a case that *moves the PC* and is dropped for `encoder` reasons is a hard error.
  That is the class of loss that quietly removes branch coverage: Keystone widening
  `ADDS R4, R1` to a 32-bit `adds.w` used to delete a whole Thumb prologue,
  including its `LDR` from the literal pool and its `POP {…, PC}` return;
* case names have to be unique.  Repeating a name with *different* content stops
  the run (one of the two would be invisible in every report keyed by name, as
  `arm_bx_r3` was for a single-insn `BX R3` and a four-insn branch-into-the-slots
  case); an exactly duplicated case is dropped and counted in `dedup N`.

## A green corpus is not the same as a covered corpus

The generator drops a case it cannot assemble and a case whose effect cannot be
observed, and both look identical to `test_instr`: a case that branches into a sea
of zero words passes for *any* target, because `movs r0, r0` changes nothing.  The
`arm_blxi_*` cases were a clean example - three of them, all assembled to the same
word (the loop variable was never used), all passing, none of them able to notice
that the BLX offset was being computed at half scale.  Deliberately putting that
bug back leaves the corpus at 350/350 while `test_unit` fails three checks.

The reverse also happens: the `encoder` rule above fired on `seq_thumb_0`, and the
sequence it had been deleting turned out to be the first case in the corpus that
loaded a *literal pool into R4-R7*.  Decoding it (`LDR R5, [PC, #0x10]`, `0x4D04`)
showed the Thumb decoder treating bit 10 of that group as an "unallocated" marker -
it is `Rt[2]`, so every `LDR R4`-`LDR R7` from a pool was being reported as
undefined, and every pool load the corpus did contain used R0-R3, which is why a
green corpus had never seen it.

The habits that follow from this: (1) check that the case *count* moved when you
add cases, and read the "skipped" list; (2) give every case slots with distinct
effects on either side of the interesting address, so an off-by-a-factor target
computation lands on a different R0; (3) where the assembler or the oracle cannot
express a case at all, hand-assemble it into `test_unit.cpp` rather than leaving a
corpus case that only proves the core does not crash.

## What the corpus cannot express

* A `PUSH`/`POP` case whose *return state* is observed.  A banked register cannot
  be handed to either model from outside: the oracle reloads the bank its mode
  caches when the PC moves, so a stack case has to give itself an SP with a guest
  `MOV SP, R4` and ends with LR reading 0.  The corpus therefore pins the round trip
  of the saved registers (that is `seq_arm_0` and `seq_thumb_0`), but the state the
  popped value selects is pinned in `test_unit.cpp`, in `test_push_pop_return()`,
  where LR is seeded odd and even.
* `BLX` with an immediate, beyond the interworking itself.  Keystone *does* use
  the ARMv5 A2 layout (`cond 1111 101 H imm23 1`) for `BLX <label>`, so the
  `arm_blxi` case is a genuine A2 instruction on this core - but the corpus can
  only say that the branch happened: the target has to be a zero word, because an
  ARM-mode line list cannot express a Thumb callee, and R14 is not compared.  A
  target that is wrong by a factor of two therefore passes.  The A2 offset
  scaling (`imm32 = SignExtend(imm24 : H : 0)`, an *even* offset, not a multiple
  of 4) and the H bit are pinned by hand-assembled words in
  `tests/test_unit.cpp::test_load_pc_and_l4`, whose two BLX phases differ only in
  insn[24] and land two bytes apart.
* `LDM`/`STM` register-list effects on the flags and on banked R13/R14, and any
  check of `R15` itself: `test_instr.cpp` compares R0-R12 and lets the branch
  target show up as a register write (`MOV Rn, R14`) instead.

## The JIT backends

One interpreter, two code generators, one translation layer.  `BRAIN_JIT_BACKEND`
(`jit/jit.h`) picks the backend from the *host*: 1 is AArch64
(`src/modules/arm926/jit/emit_arm64.h`), 2 is x86-64 (`src/modules/arm926/jit/emit_x64.cpp`), 0 means neither
is compiled in and `use_jit` degrades to the interpreter, which is what `--jit`
reports as `insns_jit 0` rather than pretending to have run anything.  On a
given host exactly one backend can run, because a block is host machine code.
Both are fed by the same `jit/frontend.cpp` and see only the IR of `jit/ir.h`,
so the accepted *instruction* set below is a property of the front end and is
the same for both - what differs is how each machine spells the result.

The accepted set is unconditional ARM-state data processing with every operand in
R0-R12 - all sixteen opcodes, RSC included - with the four barrel shifts by an
immediate (including RRX, which the emitters spell out because `ROR #0` in an AArch64
shift field means "rotate by zero" to a model), rotated immediates, and the S forms.
A block ends at a branch, a load or a store (they need the MMU and the abort path, and
a compiled block must not grow a second copy of them), at MRS/MSR, at the multiply
space, at anything naming R13-R15, at a shift amount taken from a register, at a
32-bit shift (which is what ARM state spells `LSR #0` and `ASR #0`, and AArch64's
shift field only reaches 31), and at the three shapes whose flags the host cannot
produce for free: a flag-setting rotated immediate (ARM takes C from the rotator,
where `ADDS #imm` takes it from the sum), ADCS/SBCS (whose carry-in has to be the C
of *before* the operation - a snapshot per instruction, which is exactly what RSC
does *not* need, because RSC is the platform's own reversed form), and a multiply.

The flags are the interesting part.  AArch64 has no flag-setting `ORR`/`EOR`, so
those become a value plus `TST Wd, Wd`; the 64-bit shift inside a shifted logical
produces a carry that has nothing to do with ARM's bit 32-n (or n-1); and ARM never
writes V on a logical operation at all.  So the emitter snapshots `NZCV` with `MRS`
and patches the bits back with `UBFX`/`ORR`/`MSR` - and for "unchanged" it needs the
value from *before* the instruction, which is why the snapshot is taken first.

### The x86-64 backend

Same front end, same IR, different machine to satisfy.  The shadow pointer lives
in RBP, twelve host registers (RAX RCX RDX RSI RDI R8 R9 R10 RBX R12-R14) hold
guest R0-R12 across the block, R11 is the scratch the operand barrel-shifter and
the carry repair use, and R15 holds the guest's V - because x86 writes SF, ZF and
CF cheaply but has no cheap writer for OF.  The dispatcher therefore hands the
block an *EFLAGS image* it can load with one `popfq` (`SH_EFLAGS`) and merges the
image back into `Cpu::cpsr` afterwards, keeping V in `SH_VFLAG`; `JitBlock::meta`
says which halves a given block touches.

Four x86 rules do not match ARM's, and each costs at most one instruction:

* **CF is a borrow, not a carry.**  `sub`/`sbb` leave CF inverted relative to
  ARM's C, so a flag-setting `SUB`/`SBC`/`RSB` inverts it with `cmc` before the
  exit image is built (`cmc` touches only CF, so V is still `seto`'s); an `sbb`
  *reading* the guest's carry needs the same complement first.
* **Rotates do not set SF or ZF.**  `shl`/`shr`/`sar` do - so those two costs
  nothing - but `ror`/`rcr` set CF alone, which silently loses N and Z.  The
  rotate's carry (exactly ARM's) is stashed with `setcc`, the result gets its N
  and Z from a `test`, and `bt` puts the carry back; `setcc` and `bt` touch only
  CF, so the repair cannot disturb what it is repairing.
* **Logical operations clear CF**, where ARM leaves C alone for an unshifted
  operand and takes it from the shifter otherwise: one `setcc`/`bt` pair, and
  `bt <operand>, n` for the shifter's bit, read from the *unmodified* source.
* **RSC's borrow-in has to survive a materialised shift.**  `SBC` is one `cmc`
  plus one `sbb`; RSC is the same pair with the operands exchanged, because `sbb`
  already computes `minuend - subtrahend - CF` and ARM's RSC *is* the reversed form
  (`Op2 - Rn - NOT(C)`, A4.1.61).  The wrinkle is that a shifted operand is
  materialised with a host shift before the subtraction, and a host shift destroys
  CF - the only thing the operation reads out of the flag register - so for those
  shapes the guest's carry is stashed flag-free in the emitter's scratch and put
  back with `bt`, `cmc` before the `sbb`.  `LSL #1..#3` materialises through `lea`,
  which leaves CF alone and skips the stash.
* **V has to be tracked separately.**  Since V lives in a register rather than in
  the flag register, "the flags are dead here" is not the same as "V is dead":
  an `ands` preserves V by not touching R15, so the V a later instruction passes
  to the exit can be the one a *previous* arithmetic instruction computed - the
  emitter keeps a second liveness variable (`v_live_out`) and emits `seto` for
  the arithmetic instruction whenever either variable wants it.

`tests/test_jit_x64.cpp` is the check, and it is the strongest one available:
the machine the blocks run on is the machine they were translated for, so there
is no model in the loop.  It pins the encoder byte by byte, hands a block a
sentinel-filled shadow buffer and looks at what it wrote (an offset the emitter
and `jit/abi.h` disagree about shows up as a clobbered word), sweeps every
data-processing opcode with both S settings, immediates (rotated and not), plain
registers, all four shifts and RRX across three entry flag words, then compares
thousands of random multi-instruction blocks and a register-pressure block
against the interpreter instruction by instruction.  A disagreement prints the
guest words, the entry state and the differing register, which is how the bugs
this section describes were found - the sweep found the rotate/MVN-immediate
rules, and the random blocks found the operand-swap and carry-fixup hazards the
sequential tests never reach.

`tests/test_jit_arm64.py` checks the other backend, and runs as part of `make check`:
every encoding in `tools/jit_emit --forms` is disassembled and compared to the text it
is supposed to produce (42 forms, 81 words, 0 wrong); every ARM block the corpus
contains is emitted and executed on Unicorn's AArch64 model, comparing R0-R12, CPSR
and the next PC against the interpreter (132 blocks agree, 107 are refused by design);
and `jit_emit --sweep` enumerates 16 opcodes x 4 shift types x 32 shift amounts x S x
immediate-or-register - 4224 shapes, most of which the corpus cannot express - and
runs each accepted one on the model (2805 agree, 1419 refused, 0 disagreements).  The
sweep is what found the four real bugs in the carry rule, `ROR #0`, and MVN's
immediate and RRX forms: the encoding checks had passed throughout, because a
disassembler only proves what the *text* of an instruction says.  Capstone and Unicorn
are optional here: without them the second and third layers print a `SKIP:` line
rather than pretending to have run.  `tests/test_shifter.py` belongs alongside them:
it runs every shift encoding - all four types, every amount the field holds, the
re-encoded zeros, and register-supplied amounts up to 255 - against the model, so
the primitive that both engines share stays honest even where the corpus has no
instruction that uses it (it has none), and `test_unit.cpp` pins the same rules as a
100-row table that needs nothing installed.
