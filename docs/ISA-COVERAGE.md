# ISA coverage

Status of the ARMv5TEJ / ARM926EJ-S instruction set as implemented by
`src/core/decode_arm.cpp`, `src/core/decode_thumb.cpp` and the single execution
engine in `src/core/interp_arm.cpp`. "Checked" below means a case in
`tests/data/corpus.txt` compares this core against a golden emulator for that
encoding; 303 cases exist today (169 ARM, 73 Thumb, 59 randomised multi-instruction
sequences).

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
| Coprocessor | `MRC MCR` for p15, `LDC/STC` p15, `CDP` | p15 `MRC/MCR` implemented and checked (decode *must not* turn them into undefined); `CDP` and any other coprocessor number decode as UNDEF, matching a core with no coprocessor |
| Software interrupt / debug | `SWI`, `BKPT` | implemented, checked |

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

1. A host code generator, so `Stats::insns_jit` becomes non-zero and the corpus
   can be replayed through the JIT.  `test_instr --jit` sets `cfg.use_jit` and
   compares results, which on a host without a backend is a relabelled interpreter
   run; the summary line now reports the engine that actually retired
   instructions (`insns_jit`) instead of trusting the flag.
2. `LDC/STC` for p15 (the ARM926EJ-S uses these for a handful of cache/debug
   operations), and unaligned accesses to `PG_DEVICE` pages.
3. No way to drive an interrupt.  The core keeps `irq_line`/`fiq_line` but the
   public API has nothing that raises them, so IRQ and FIQ *entry* is untested:
   `test_exception_entry()` covers the three exceptions a guest instruction can
   raise (undefined instruction, SWI, external data abort) in both states and for
   both vector bases, and the FIQ register window is pinned by
   `test_exception_banking()`, but the FIQ-only `F` masking and the 8/6 link
   adjustment for IRQ/FIQ have no test until something can assert the lines.
4. Prefetch-abort details for a fetch from a device page.  (The `L4` half of this
   item is closed: `Cpu::load_pc()` honours SCTLR.L4 for `LDR`/`LDM`/`POP` into R15
   as well as for the `MOV`-style PC writes, c1[15] is writable and reads back, and
   `tests/test_unit.cpp` pins both settings.)

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
bug back leaves the corpus at 341/341 while `test_unit` fails three checks.

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
