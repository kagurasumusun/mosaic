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
| Conditional branch, SWI | `BEQ..BVC`, `SWI` | implemented, checked |
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
   can be replayed through the JIT.
2. `LDC/STC` for p15 (the ARM926EJ-S uses these for a handful of cache/debug
   operations), and unaligned accesses to `PG_DEVICE` pages.
3. Prefetch-abort details for a fetch from a device page.  (The `L4` half of this
   item is closed: `Cpu::load_pc()` honours SCTLR.L4 for `LDR`/`LDM`/`POP` into R15
   as well as for the `MOV`-style PC writes, c1[15] is writable and reads back, and
   `tests/test_unit.cpp` pins both settings.)

## What the corpus cannot express

* `BLX` with an immediate.  The encoding this core implements is the ARMv5 one,
  `cond 1111 101 H imm23 1` with offset `SignExtend(imm23 : H : 10)`.  Keystone
  assembles `BLX <label>` as the ARMv6+ `cond 101 H imm24` form, which on an
  ARM926EJ-S decodes as a plain `B`, so a corpus line written as `BLX #imm` would
  silently test a different instruction on the two models.  The corpus therefore
  covers `BX`/`BLX` with the target in a register only; the immediate form has to
  be hand-assembled, which is what `tests/test_unit.cpp` does.
* `LDM`/`STM` register-list effects on the flags and on banked R13/R14, and any
  check of `R15` itself: `test_instr.cpp` compares R0-R12 and lets the branch
  target show up as a register write (`MOV Rn, R14`) instead.
