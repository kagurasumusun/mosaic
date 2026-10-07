/* AArch64 code generation for the JIT's IR.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The emitter is a plain function that appends 32-bit words to a buffer, so it can
 * be exercised on any host: tests/test_jit_arm64.py feeds the words to Capstone
 * (does each encoding mean what we think it means) and to Unicorn (does the block
 * compute what the interpreter computes) without needing an AArch64 toolchain.
 *
 * Register use, fixed for every block - the ABI a backend would have to keep:
 *
 *   X0  pointer to the shadowed guest state, laid out as
 *         u32 regs[16]; u32 cpsr;              (guest address 0x1000 today)
 *   X1..X13  guest R0..R12 while the block runs
 *   X14, X15 scratch (constants, the CPSR word at entry and exit)
 *
 * The guest's N, Z, C and V live in the host NZCV for the duration of a block:
 * both ISAs put them at bits[31:28] and both set them from the S bit of an
 * operation, so `adds`/`subs`/`ands` need no fixup and the exchange is two
 * instructions.  Q, A and the mode/interrupt bits are not reachable from any op a
 * block contains, so they are carried through untouched by merging into the memory
 * word at the end, and R15 becomes the address the block stopped at.
 *
 * Encodings are from DDI 0487 C6.1.  Only the shifted-register and
 * add/subtract-immediate forms are used for the ALU, plus MOVZ/MOVK for constants:
 * the logical-immediate form needs a bitmask-decoding step that is not worth it
 * when a scratch register is two instructions.
 */
#ifndef BRAIN_JIT_EMIT_ARM64_H
#define BRAIN_JIT_EMIT_ARM64_H

#include <cstdint>
#include <vector>

#include "arm926/jit/ir.h"

namespace brain::arm926 {
namespace arm64 {

enum : uint32_t {
  XZR = 31,
  FP = 0,                 /* the pointer to the shadowed guest state      */
  SCR1 = 14,
  SCR2 = 15,
  SCR3 = 16,              /* extra scratch; X16/X17 are free in the ABI   */
  CPSR_OFF = 16 * 4,      /* regs[16] is the CPSR word by layout          */
  R15_OFF = 15 * 4,
  FLAGS_MASK = 0xF0000000u,
};

enum : uint32_t { SH_LSL = 0, SH_LSR = 1, SH_ASR = 2, SH_ROR = 3 };

class Buf {
 public:
  std::vector<uint32_t> w;

  void emit(uint32_t x) { w.push_back(x); }

  /* ---- memory --------------------------------------------------------- */
  /* LDR/STR (immediate, unsigned offset).  size = 10 selects the 32-bit form
   * (0xB9400000; 0x39400000 is LDRB) and the 10-bit field at [21:10] holds the
   * offset divided by the access size, so the scaling has to stay. */
  void ldr32(uint32_t rt, uint32_t rn, uint32_t off) {
    emit(0xB9400000u | (((off >> 2) & 0x3FFu) << 10) | (rn << 5) | rt);
  }
  void str32(uint32_t rt, uint32_t rn, uint32_t off) {
    emit(0xB9000000u | (((off >> 2) & 0x3FFu) << 10) | (rn << 5) | rt);
  }

  /* ---- constants ------------------------------------------------------ */
  void movz(uint32_t rd, uint32_t imm16, uint32_t hw = 0) {
    emit(0x52800000u | ((hw & 3u) << 21) | ((imm16 & 0xFFFFu) << 5) | rd);
  }
  void movk(uint32_t rd, uint32_t imm16, uint32_t hw) {
    emit(0x72800000u | ((hw & 3u) << 21) | ((imm16 & 0xFFFFu) << 5) | rd);
  }
  void mov_imm(uint32_t rd, uint32_t v) {
    movz(rd, v & 0xFFFFu, 0);
    if (v >> 16) movk(rd, (v >> 16) & 0xFFFFu, 1);
  }

  /* ---- logical, shifted-register form ----
   *
   * `sf op1 01010 N imm6 Rm shift Rn Rd` is not the layout; the group shares the
   * add/subtract one: shift at [23:22], N (the invert bit) at [21], Rm at [20:16],
   * the shift amount at [15:10], Rn at [9:5], Rd at [4:0].  op1 selects the
   * operation, and unlike most of AArch64 the S variants here - ANDS and BICS -
   * *do* keep a destination, so a flag-setting AND or BIC is one instruction.
   * There is no ORRS or EORS at all: an OR/EOR/MOV/MVN that has to leave flags
   * behind writes its value first and raises N and Z with `TST Wd, Wd`. */
  static constexpr uint32_t kLogicalN = 1u << 21;
  void logic(uint32_t base, uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh,
             uint32_t amt) {
    emit(base | (sh << 22) | (rm << 16) | (amt << 10) | (rn << 5) | rd);
  }
  void and_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0, bool flags = false) {
    logic(flags ? 0x6A000000u : 0x0A000000u, rd, rn, rm, sh, amt);
  }
  void bic_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0, bool flags = false) {
    logic(flags ? (0x6A000000u | kLogicalN) : (0x0A000000u | kLogicalN), rd, rn, rm,
          sh, amt);
  }
  void orr_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0) {
    logic(0x2A000000u, rd, rn, rm, sh, amt);
  }
  void eor_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0) {
    logic(0x4A000000u, rd, rn, rm, sh, amt);
  }
  /* N and Z from a register, and nothing else: `ANDS WZR, Wd, Wd` is `TST Wd, Wd`,
   * whose result is Wd itself.  It leaves C and V at zero, which is why ARM's
   * logical operations need flags_patch() afterwards. */
  void set_nz_from(uint32_t rd) { emit(0x6A000000u | (rd << 16) | (rd << 5) | XZR); }
  void orn_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0) {
    emit(0x2A200000u | (sh << 22) | (rm << 16) | (amt << 10) | (rn << 5) | rd);
  }
  void mov_reg(uint32_t rd, uint32_t rm) { orr_shifted(rd, XZR, rm); }
  void mvn(uint32_t rd, uint32_t rm) { orn_shifted(rd, XZR, rm); }
  /* `ORR Rd, WZR, Rm, <sh> #amt` is AArch64's shift-by-a-constant: the bits
   * shifted out are dropped, which is what the guest's LSR/ASR/RRX do too. */
  void shift(uint32_t rd, uint32_t rm, uint32_t sh, uint32_t amt) {
    if (amt == 0 && sh != SH_ROR) { mov_reg(rd, rm); return; }
    orr_shifted(rd, XZR, rm, sh, amt);
  }

  /* ---- add/subtract --------------------------------------------------- */
  void add_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0, bool flags = false) {
    emit((flags ? 0x2B000000u : 0x0B000000u) | (sh << 22) | (rm << 16) |
         (amt << 10) | (rn << 5) | rd);
  }
  void sub_shifted(uint32_t rd, uint32_t rn, uint32_t rm, uint32_t sh = SH_LSL,
                   uint32_t amt = 0, bool flags = false) {
    emit((flags ? 0x6B000000u : 0x4B000000u) | (sh << 22) | (rm << 16) |
         (amt << 10) | (rn << 5) | rd);
  }
  void add_imm(uint32_t rd, uint32_t rn, uint32_t imm12, bool flags = false) {
    emit((flags ? 0x31000000u : 0x11000000u) | ((imm12 & 0xFFFu) << 10) |
         (rn << 5) | rd);
  }
  void sub_imm(uint32_t rd, uint32_t rn, uint32_t imm12, bool flags = false) {
    emit((flags ? 0x71000000u : 0x51000000u) | ((imm12 & 0xFFFu) << 10) |
         (rn << 5) | rd);
  }
  void neg_shifted(uint32_t rd, uint32_t rm, uint32_t sh = SH_LSL, uint32_t amt = 0,
                   bool flags = false) {
    sub_shifted(rd, XZR, rm, sh, amt, flags);
  }
  void adc(uint32_t rd, uint32_t rn, uint32_t rm) {
    emit(0x1A000000u | (rm << 16) | (rn << 5) | rd);
  }
  /* Subtract with carry.  Unlike the shifted-register group, add/subtract
   * *with carry* has no shift field - an operand that needs one is shifted
   * separately - so this takes plain registers, and the flag-setting form
   * (`SBCS`, which is what makes RSC one instruction on this host) is the only
   * variation.  AArch64's SBC is also already the reversed form ARM's RSC
   * wants: `SBCS Wd, Wm, Wn` is Wm - Wn - NOT(C) with N, Z, C and V. */
  void sbc(uint32_t rd, uint32_t rn, uint32_t rm, bool flags = false) {
    emit((flags ? 0x7A000000u : 0x5A000000u) | (rm << 16) | (rn << 5) | rd);
  }

  /* ---- flags ------------------------------------------------------------ */
  /* MRS/MSR of the NZCV register; both keep Rt in bits[4:0].  `MSR NZCV, Xt`
   * reads only bits[31:28] of Xt, so no masking is needed on the way in. */
  void mrs_nzcv(uint32_t rt) { emit(0xD53B4200u | rt); }
  void msr_nzcv(uint32_t rt) { emit(0xD51B4200u | rt); }

  void ret() { emit(0xD65F03C0u); }
  void nop() { emit(0xD503201Fu); }

  /* ---- a block's entry and exit --------------------------------------- */
  /* Every shadowed register is loaded whether the block uses it or not: the
   * loads are sequential, and the alternative is per-block liveness analysis
   * for two instructions' worth of saving. */
  /* `ubfx Wd, Wn, #lsb, #width` = UBFM with imsb = lsb + width - 1. */
  void ubfx(uint32_t rd, uint32_t rn, uint32_t lsb, uint32_t width) {
    /* UBFM Wd, Wn, #lsb, #(lsb+width-1), i.e. the Bitfield Move group with sf=0:
     * bits[31:22] = 0 1 10 01 1 0 0 and then immr[21:16], imsb[15:10], Rn, Rd,
     * which puts the base at 0x53000000 (0x13000000 in the same shape is SBFM,
     * i.e. `sbfx` - one bit away and the sign gets pulled in instead). */
    emit(0x53000000u | ((lsb + width - 1u) << 10) | (lsb << 16) | (rn << 5) | rd);
  }
  /* Patch a field of the *host* flags from a field of a source register.
   *
   * ARM's flag rules and AArch64's agree for the arithmetic cases but not for
   * `C`/`V` on logical operations or for `C` when an arithmetic operand was
   * barrel-shifted: ARM leaves V alone for AND/ORR/EOR/BIC/MOV/MVN, takes C from
   * the shifter rather than the sum, and does not touch C at all when there was no
   * shift.  AArch64 has no way to express "this flag is unchanged" - any flag-
   * setting form writes all four - so the bits that must survive are rewritten from
   * the register holding them, and every step here is flag-free by construction:
   * a flag-setting op would replace the very flags it is meant to repair.
   *
   * `mask` selects the NZCV bits to overwrite (bit 31 N, 30 Z, 29 C, 28 V), `pos`
   * is its lowest set bit, and the source field is `width` bits at `lsb`.  The
   * source for a bit ARM leaves alone is the snapshot of the host flags taken just
   * *before* the operation - not the CPSR the block came in with: inside a block the
   * flags are live, and a previous instruction may well have changed them. */
  void flags_patch(uint32_t mask, uint32_t pos, uint32_t src, uint32_t lsb,
                   uint32_t width) {
    /* SCR1 takes the current flags, SCR2 the scratch mask - and deliberately not
     * SCR3, which holds the snapshot this is reading, and may have to survive for a
     * second patch.  Every destination is therefore also a source exactly once. */
    mrs_nzcv(SCR1);
    mov_imm(SCR2, ~mask);
    and_shifted(SCR1, SCR1, SCR2);
    ubfx(SCR2, src, lsb, width);
    orr_shifted(SCR1, SCR1, SCR2, SH_LSL, pos);
    msr_nzcv(SCR1);
  }
  /* The same repair when the value to patch in is a *constant* the translation
   * already knows - the bit a rotated immediate's rotator shifted out, which is
   * not in any register by the time the operation has run (the value operand is
   * materialised, but the operation may overwrite that register, and the patch
   * needs a third scratch the block does not have). */
  void flags_patch_imm(uint32_t mask, uint32_t bits) {
    mrs_nzcv(SCR1);
    mov_imm(SCR2, ~mask);
    and_shifted(SCR1, SCR1, SCR2);
    mov_imm(SCR2, bits & mask);
    orr_shifted(SCR1, SCR1, SCR2);
    msr_nzcv(SCR1);
  }
  /* `MOV Rd, Rm, RRX`: the bit entering the top of the result is the carry that is
   * current *right now*, which is what makes it different from `ROR #1` - so read it
   * out of the host flags rather than any saved copy. */
  void mov_rrx(uint32_t rd, uint32_t rm) {
    mrs_nzcv(SCR2);
    ubfx(SCR2, SCR2, 29, 1);              /* C, right-aligned */
    ubfx(rd, rm, 1, 31);                  /* LSR #1 */
    orr_shifted(rd, rd, SCR2, SH_LSL, 31);
  }
  void prologue() {
    for (uint32_t i = 0; i < 13; ++i) ldr32(1 + i, FP, 4 * i);
    ldr32(SCR1, FP, CPSR_OFF);
    msr_nzcv(SCR1);
  }
  /* Writes back only the shadowed registers the block changed, merges the host
   * flags into the CPSR word, and leaves R15 at `next_pc`.  The merge has to
   * mask: `MRS Xt, NZCV` also returns Q and C (bits 27:26) of the host's
   * saturating-accumulation state, which are not the guest's Q and A. */
  void epilogue(uint32_t next_pc, uint32_t dirty) {
    mov_imm(SCR3, FLAGS_MASK);
    mrs_nzcv(SCR1);
    ldr32(SCR2, FP, CPSR_OFF);
    bic_shifted(SCR2, SCR2, SCR3);            /* drop the saved NZCV        */
    and_shifted(SCR1, SCR1, SCR3);            /* keep only the host's       */
    orr_shifted(SCR2, SCR2, SCR1);
    str32(SCR2, FP, CPSR_OFF);
    for (uint32_t i = 0; i < 13; ++i)
      if (dirty & (1u << i)) str32(1 + i, FP, 4 * i);
    mov_imm(SCR2, next_pc);
    str32(SCR2, FP, R15_OFF);
    ret();
  }
};

}  // namespace arm64
}  // namespace brain::arm926

#endif
