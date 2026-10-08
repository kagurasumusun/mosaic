/* brain-emu -- ARMv5TEJ ISA: ARM (32-bit) decoder.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * One decoder serves both engines: the interpreter executes what it returns,
 * the JIT translates it (and falls back to "run the interpreter for this
 * instruction" for the handful of classes it does not inline).  Sharing the
 * decoder is what makes the two execution paths agree by construction.
 *
 * Bit patterns come from ARM DDI 0100I (reference/DDI01001.pdf), chapter A4/A5
 * "Instruction encodings"; every pattern below is exercised by
 * tests/tools/check_decoder.py, which cross-checks this decoder against
 * capstone's disassembly of the same words.
 *
 * Summary of the tree implemented here:
 *
 *  bits[27:26]==00 bit[24]==0  data processing / multiply / post-indexed
 *                              "extra" load-store (halfword, signed, LDRD)
 *  bits[27:26]==00 bit[24]==1  MRS MSR BX BLX BXJ BKPT CLZ SWP
 *                              QADD QSUB QDADD QDSUB SMLAxy SMULxy SMLALxy
 *                              SMLAWy SMULWy, pre-indexed extra load-store
 *  bits[27:26]==01            LDR STR LDRB STRB (immediate or shifted register)
 *  bits[27:25]==100           LDM STM
 *  bits[27:25]==101           B BL ; BLX #imm uses cond==1111
 *  bits[27:25]==110           LDC STC MCR MRC (MCR2/MRC2 need opcode1!=0)
 *  bits[27:24]==1111          SWI
 */
#include "arch/arm/isa/armv5/decode.h"

namespace brain::arm::isa::armv5 {

#define BF(insn, hi, lo) (((insn) >> (lo)) & ((1u << ((hi) - (lo) + 1)) - 1u))
#define BIT(insn, n) BF(insn, n, n)

/* operand2 of a data-processing instruction (bits 11:0, I=0) */
static inline void decode_op2_reg(u32 insn, ArmDec* d) {
  d->sh = BF(insn, 6, 5);
  if (BIT(insn, 4)) {                 /* shifted by a register */
    d->op2 = OP2_REG_RS;
    d->rs = BF(insn, 11, 8);
    return;
  }
  /* The five-bit shift amount cannot say #32, so ARM state re-encodes a zero
   * field: for the two right-ward shifts it means a 32-bit shift ("the form of
   * the shift field which might be expected to correspond to LSR #0 is used to
   * encode LSR #32, which has a zero result with bit 31 of Rm as the carry
   * output" - ARM DDI 0084D s4.5.2, the same rule as DDI 0406 A1.8.1), and for
   * ROR it means RRX.  Only LSL #0 is really "no shift", and it leaves C alone,
   * which is what OP2_REG below says.  A register-supplied amount is *not*
   * re-encoded: `LSR Rm, R0` with R0 = 0 is a shift by zero (DDI 0406 A1.8.1's
   * note that the #0 spellings are an assembler convenience for LSL only is the
   * same rule seen from the other side).  The Thumb-1 group already did this. */
  const u32 amt = BF(insn, 11, 7);
  if (amt) { d->op2 = OP2_REG_SHIFT; d->amt = amt; }
  else if (d->sh == SH_LSL) { d->op2 = OP2_REG; }
  else if (d->sh == SH_ROR) { d->op2 = OP2_REG_SHIFT; d->sh = SH_RRX; }
  else { d->op2 = OP2_REG_SHIFT; d->amt = 32; }
}

/* the "extra" load/store space: LDRH STRH LDRSB LDRSH LDRD STRD.
 * bits[23:20] = U,I,W,L (bit 24 is P), bits[7:4] = 1,S,H,1 for the
 * halfword/signed forms and 1101/1111 with L==0 for LDRD/STRD. */
static inline u32 decode_extra_ldst(u32 insn, ArmDec* d, u32 p) {
  const u32 b7_4 = BF(insn, 7, 4);
  const u32 U = BIT(insn, 23), I = BIT(insn, 22), W = BIT(insn, 21),
            L = BIT(insn, 20);
  d->p = p; d->u = U; d->w = W; d->l = L;
  d->am = !p ? (I ? AM_IMM_POST : AM_REG_POST)
             : (W ? (I ? AM_IMM_PRE_WB : AM_REG_PRE_WB)
                  : (I ? AM_IMM_PRE : AM_REG_PRE));
  if (I) {
    d->imm12 = (BF(insn, 11, 8) << 4) | BF(insn, 3, 0);
  } else {
    /* Register form: [Rn, Rm] with bits[11:8] SBZ (A5.3.1).  Not zero means
     * the encoding is unallocated - and the field really is a *reserved* one,
     * not an ignored one: Unicorn (and the ARM ARM) reject every shape of this
     * space with bits[11:8] != 0, halfword and doubleword alike.  Accepting it
     * silently turned `e1b911b5` into a load of [r9, r5], which the oracle
     * calls an invalid instruction. */
    if (BF(insn, 11, 8) != 0) return AOP_UNDEF;
    d->op2 = OP2_REG;                    /* [Rn, Rm] : unshifted index        */
  }

  if (b7_4 == 0b1101 || b7_4 == 0b1111) {
    if (!L) {                                  /* LDRD (1101) / STRD (1111) */
      d->op = AOP_LDRD;
      d->kind = (b7_4 == 0b1111) ? 1 : 0;      /* 1 = store                */
      return AOP_LDRD;
    }
  }
  if ((b7_4 & 0b1001) != 0b1001) return AOP_UNDEF;
  const u32 S = BIT(insn, 6), H = BIT(insn, 5);
  if (!S && !H) return AOP_UNDEF;              /* 1001 belongs to SWP/muls  */
  d->op = AOP_LDRSTH;
  d->h = H;
  d->signed_ = S;
  /* ARMv5 permits an unaligned address for the non-writeback, non-PC forms.
   * The "unaligned" flag here records that the *encoding* is one of those;
   * whether the access actually faults is decided by Cpu::check_alignment. */
  /* A5.3.2/A5.3.3: with P=1,W=0 and Rd!=R15 an unaligned address is permitted
   * (the low bits are ignored) for the halfword/signed family, for stores as
   * well as loads. */
  if (!W && d->rd != 15) d->unaligned_ok = 1;
  return AOP_LDRSTH;
}

/* the misc space with bit[24]==1: MRS MSR BX BLX BXJ BKPT CLZ SWP QD* SMLA* */
static inline u32 decode_misc(u32 insn, ArmDec* d) {
  const u32 pre = BF(insn, 27, 20);
  const u32 b7_4 = BF(insn, 7, 4);

  /* SWP / SWPB: cond 0001 0B00 Rn Rd 0000 1001 Rm  (bit[22] = B) */
  if ((pre & 0b11111011) == 0b00010000 && b7_4 == 0b1001 && !BIT(insn, 21) &&
      !BIT(insn, 20)) {
    d->op = AOP_SWP;
    d->b = BIT(insn, 22);
    return AOP_SWP;
  }
  /* CLZ: cond 0001 0110 1111 Rd 1111 0001 Rm -- tested before MRS/MSR, whose
   * pre patterns overlap it. */
  if (pre == 0x16 && BF(insn, 19, 16) == 0xF && BF(insn, 11, 4) == 0xF1) {
    d->op = AOP_CLZ;
    d->rd = BF(insn, 15, 12);
    d->rm = insn & 0xF;
    return AOP_CLZ;
  }
  /* MRS: cond 0001 0R00 1111 Rd 0000 0000 0000  (bit[22] = R: 0 CPSR 1 SPSR) */
  if ((pre & 0b11111011) == 0b00010000 && BF(insn, 19, 16) == 0xF &&
      BF(insn, 11, 0) == 0) {
    d->op = AOP_MRS;
    d->psr = BIT(insn, 22);
    return AOP_MRS;
  }
  /* MSR: cond 00 I 1 R 0 0 10 field_mask 1111 (rotate#imm8 | 0000 0000 Rm) */
  if ((pre == 0x12 || pre == 0x32 || pre == 0x16 || pre == 0x36) &&
      BF(insn, 15, 12) == 0xF) {
    if (pre == 0x12 && BF(insn, 19, 8) == 0xFFF) {
      /* BX / BLX(register) / BXJ.  The whole 19:8 field being ones is only the
       * branch space when the *register* form of MSR is what this could
       * otherwise be (bit 25 clear, bit 22 clear): the immediate form spells a
       * legal MSR as mask 1111, Rd 1111, rotate 1111 - `msr cpsr_fsxc, #rot,
       * #imm` - and treating that as a branch turns a system-register write
       * into a jump to whatever Rm happened to hold. */
      if (b7_4 == 0b0001) { d->op = AOP_BX; d->kind = 0; return AOP_BX; }
      if (b7_4 == 0b0011) { d->op = AOP_BX; d->kind = 1; return AOP_BX; }
      if (b7_4 == 0b0010) { d->op = AOP_BX; d->kind = 2; return AOP_BX; }
      return AOP_UNDEF;
    }
    if (b7_4 == 0b0111 && pre == 0x12) {          /* BKPT */
      d->op = AOP_BKPT;
      d->imm24 = (BF(insn, 19, 8) << 4) | BF(insn, 3, 0);
      return AOP_BKPT;
    }
    d->op = AOP_MSR;
    d->psr = BIT(insn, 22);
    d->mask = BF(insn, 19, 16);
    if (BIT(insn, 25)) {
      d->op2 = OP2_IMM;
      d->rot = BF(insn, 11, 8);
      d->imm12 = BF(insn, 7, 0);
    } else {
      d->op2 = OP2_REG;
    }
    return AOP_MSR;
  }
  /* QDSP space: cond 0001 0op0 Rn Rd 0000 0101 Rm */
  if (BF(insn, 27, 25) == 0b000 && BIT(insn, 24) && !BIT(insn, 23) &&
      !BIT(insn, 20) && b7_4 == 0b0101) {
    d->op = AOP_QD;
    d->kind = BF(insn, 22, 21);      /* 00 QADD 01 QSUB 10 QDADD 11 QDSUB */
    d->rm = BF(insn, 19, 16);        /* the QDSP fields are laid out the  */
    d->rn = insn & 0xF;              /* other way round from plain DP!    */
    return AOP_QD;
  }
  if (b7_4 == 0b1000 || b7_4 == 0b1010 || b7_4 == 0b1100 || b7_4 == 0b1110) {
    const u32 yx = BF(insn, 6, 5);
    switch (pre) {
      case 0x10:   /* SMLAxy: cond 0001 0000 Rd Rn Rs 1yx0 Rm */
        d->op = AOP_MULH;
        d->kind = yx;
        d->rd = BF(insn, 19, 16);          /* Rd   */
        d->ra = BF(insn, 15, 12);          /* Rn:  accumulate operand */
        return AOP_MULH;
      case 0x16:   /* SMULxy: cond 0001 0110 Rd SBZ Rs 1yx0 Rm */
        d->op = AOP_MULH;
        d->kind = yx;
        d->rd = BF(insn, 19, 16);
        d->ra = 0x1F;                      /* marker: no accumulator    */
        return AOP_MULH;
      case 0x14:   /* SMLALxy: cond 0001 0100 RdHi RdLo Rs 1yx0 Rm */
        d->op = AOP_MULHAL;
        d->kind = yx;
        d->ra = BF(insn, 19, 16);          /* RdHi; d->rd holds RdLo */
        return AOP_MULHAL;
      case 0x12: { /* SMLAWy (1y01) / SMULWy (1y11): Rd is bits[19:16] */
        d->op = AOP_MULW;
        d->rd = BF(insn, 19, 16);
        d->kind = BIT(insn, 6) | (BIT(insn, 5) << 1);   /* bit0=y, bit1=word */
        if (BIT(insn, 5)) d->ra = 0x1F;                  /* no accumulate    */
        else d->ra = BF(insn, 15, 12);
        return AOP_MULW;
      }
      default: break;
    }
  }
  /* Data processing, opcodes 8..15 (TST TEQ CMP CMN ORR MOV BIC MVN).
   * These share bits[27:25] == 000 with the group above and differ only in
   * bit[24] being 1, so anything the special encodings above did not claim is
   * plain data processing:  cond 0001 1 ooo S Rn Rd operand2  (ooo = dp - 8).
   * TST/TEQ/CMP/CMN always set the flags, so their S==0 form is unallocated. */
  if (!BIT(insn, 27) && !BIT(insn, 26) && BIT(insn, 24)) {
    const u32 dp = 8 | BF(insn, 23, 21);
    if (dp < 12 && !BIT(insn, 20)) return AOP_UNDEF;
    d->op = AOP_DP;
    d->dp = dp;
    d->s = BIT(insn, 20);
    if (BIT(insn, 25)) {
      d->op2 = OP2_IMM;
      d->rot = BF(insn, 11, 8);
      d->imm12 = BF(insn, 7, 0);
    } else {
      decode_op2_reg(insn, d);
    }
    return AOP_DP;
  }
  return AOP_UNDEF;
}

u32 decode_arm(u32 insn, ArmDec* d) {
  d->insn = insn;
  d->rd = BF(insn, 15, 12);
  d->rn = BF(insn, 19, 16);
  d->rm = insn & 0xF;
  d->rs = BF(insn, 11, 8);
  d->ra = 0;
  d->s = 0;
  d->dp = 0;
  d->op2 = OP2_NONE;
  d->rot = 0;
  d->sh = 0;
  d->amt = 0;
  d->am = AM_NONE;
  d->u = d->b = d->h = d->signed_ = 0;
  d->w = d->l = d->p = 0;
  d->imm12 = 0;
  d->imm24 = 0;
  d->list = 0;
  d->psr = 0;
  d->mask = 0;
  d->cp = 0;
  d->crn = 0;
  d->crm = 0;
  d->opc1 = 0;
  d->opc2 = 0;
  d->offset8 = 0;
  d->kind = 0;
  d->unaligned_ok = 0;

  const u32 b27_26 = BF(insn, 27, 26);

  /* ============================ bits[27:26] == 00 ==================== */
  if (b27_26 == 0) {
    const u32 b7_4 = BF(insn, 7, 4);
    const u32 b4 = BIT(insn, 4), b7 = BIT(insn, 7);

    if (!BIT(insn, 24)) {
      /* post-indexed extra load-store (P==0) shares this space */
      if (!BIT(insn, 25) && b4 && b7 && b7_4 != 0b1001)
        return decode_extra_ldst(insn, d, 0);

      if (b4 && b7 && !BIT(insn, 25)) {
        /* ---- multiply space: bits[7:4] == 1001, bits[25:24] == 00 - */
        const u32 b23_21 = BF(insn, 23, 21);
        if (!BIT(insn, 23) && b23_21 <= 1) {
          d->op = AOP_MUL;
          d->s = BIT(insn, 20);
          d->kind = b23_21 & 1;              /* A: MLA */
          d->rd = BF(insn, 19, 16);
          d->rn = BF(insn, 15, 12);          /* Rn = accumulate value */
          return AOP_MUL;
        }
        if (BIT(insn, 23)) {
          d->op = AOP_MULL;
          d->kind = BF(insn, 22, 21);        /* 00 UMULL 01 UMLAL 10 SMULL 11 SMLAL */
          d->s = BIT(insn, 20);
          d->ra = BF(insn, 19, 16);          /* RdHi */
          d->rd = BF(insn, 15, 12);          /* RdLo */
          return AOP_MULL;
        }
        return AOP_UNDEF;
      }
      /* bits[7:4] only carries meaning for the *shifted register* operand2: bit[4]
       * selects the register form and bit[7] (RES0 there, since Rs lives in [11:8])
       * must read 0.  A data-processing *immediate* (I == 1) has no such field -
       * bit[4] is simply part of imm8 - so interrogating it there rejected one
       * immediate in sixteen: ANDS Rd, Rn, #0xFF decoded as undefined. */
      if (BIT(insn, 25) && b4) { /* immediate operand2: bits[7:4] are data */ }
      else if (b4 && b7) return AOP_UNDEF;

      /* ---- data processing: cond 00 I 0 opcode S Rn Rd operand2 ------- */
      d->op = AOP_DP;
      d->s = BIT(insn, 20);
      d->dp = BF(insn, 23, 21);
      if (BIT(insn, 25)) {
        d->op2 = OP2_IMM;
        d->rot = BF(insn, 11, 8);
        d->imm12 = BF(insn, 7, 0);
      } else {
        decode_op2_reg(insn, d);
      }
      return AOP_DP;
    }

    /* bit[24] == 1: pre-indexed extra load-store, or the misc space */
    if (!BIT(insn, 25) && b4 && b7 && b7_4 != 0b1001)
      return decode_extra_ldst(insn, d, 1);
    return decode_misc(insn, d);
  }

  /* ============================ bits[27:26] == 01 ==================== *
   * LDR/STR/LDRB/STRB: cond 01 I P U B W L Rn Rd [offset]               */
  if (b27_26 == 1) {
    const u32 I = BIT(insn, 25), P = BIT(insn, 24), U = BIT(insn, 23),
              B = BIT(insn, 22), W = BIT(insn, 21), L = BIT(insn, 20);
    d->op = AOP_LDRST;
    d->p = P; d->u = U; d->b = B; d->w = W; d->l = L;
    if (!I) {
      d->imm12 = BF(insn, 11, 0);
      d->am = !P ? AM_IMM_POST : (W ? AM_IMM_PRE_WB : AM_IMM_PRE);
      if (P && !W && d->rd != 15) d->unaligned_ok = 1;
    } else {
      /* `decode_op2_reg` sorts out which of the three register-offset shapes
       * this is (shift by imm5, register-controlled shift, or a plain register
       * when bits[6:4] read zero) - see decode_op2_reg. */
      decode_op2_reg(insn, d);
      d->am = !P ? AM_REG_POST : (W ? AM_REG_PRE_WB : AM_REG_PRE);
    }
    return AOP_LDRST;
  }

  /* ============================ LDM / STM =========================== *
   * cond 100 P U S W L Rn register_list                                */
  if (BF(insn, 27, 25) == 0b100) {
    d->p = BIT(insn, 24);
    d->u = BIT(insn, 23);
    d->kind = BIT(insn, 22);      /* S: user-bank / CPSR restore        */
    d->w = BIT(insn, 21);
    d->l = BIT(insn, 20);
    d->list = BF(insn, 15, 0);
    d->op = d->l ? AOP_LDM : AOP_STM;
    return d->op;
  }

  /* ============================ B / BL / BLX imm ==================== *
   * B/BL   : cond 101 L imm24            (offset = SignExtend(imm24:00)) *
   * BLX imm: 1111 101 H imm23 1    (offset = SignExtend(imm24:H:0), imm24[0]=1) */
  if (BF(insn, 27, 25) == 0b101) {
    const u32 cond = BF(insn, 31, 28);
    if (cond == 0xF) {
      /* A4.1.9: the whole cond=1111 half of this opcode space is BLX
       * (immediate) - there is no "NV branch".  `imm32 = SignExtend(imm24:H:0,
       * 32)` with imm24 = insn[23:0], so the offset is *even* but not
       * necessarily a multiple of 4 (that is what lets one encoding reach both
       * 0x...0 and 0x...2 through H), and reading the immediate as insn[23:1]
       * would halve every displacement - `0xFA000002` (which an assembler
       * emits for `blx` to pc+16) would land 4 bytes short. */
      s32 off = (s32)(((insn & 0x00FFFFFFu) << 2) | (BIT(insn, 24) << 1));
      if (off & 0x02000000) off |= (s32)0xFC000000u;      /* sign extend @26 */
      d->imm24 = (u32)off;
      d->kind = 2;                     /* BLX: branch and exchange        */
      d->op = AOP_B;
      return AOP_B;
    }
    s32 off = (s32)(BF(insn, 23, 0) << 8) >> 8;           /* sign extend @24 */
    /* imm24:00.  Scaling happens in the unsigned domain: `off << 2` with a
     * negative `off` is undefined, and the two bit patterns are identical. */
    d->imm24 = (u32)off << 2;
    d->kind = BIT(insn, 24) ? 1 : 0;   /* 0 = B, 1 = BL                   */
    d->op = AOP_B;
    return AOP_B;
  }

  /* ============================ coprocessor / SWI =================== *
   * cond 1111 imm24                                     SWI            *
   * cond 1110 opcode1 0 CRn CRd cp_num 0 opcode2 CRm    CDP           *
   * cond 1110 opcode1 L CRn Rd  cp_num 1 opcode2 CRm    MCR/MRC       *
   * cond 110PUN0WL Rn CRd cp_num offset_8                LDC/STC       *
   * (the ARM926EJ-S has no coprocessor, so CDP/LDC/STC/MCR/MRC all     *
   * take an Undefined Instruction exception; CP15 register access uses *
   * MRC/MCR with cp==15.)                                              */
  const u32 b27_24 = BF(insn, 27, 24);
  if (b27_24 == 0b1111) {
    d->op = AOP_SWI;
    d->imm24 = BF(insn, 23, 0);
    return AOP_SWI;
  }
  if (b27_24 == 0b1110) {
    if (!BIT(insn, 4)) return AOP_UNDEF;      /* CDP: no coprocessor    */
    d->cp = BF(insn, 11, 8);
    d->crn = BF(insn, 19, 16);
    d->crm = BF(insn, 3, 0);
    d->opc1 = BF(insn, 23, 21);
    d->opc2 = BF(insn, 7, 5);
    d->l = BIT(insn, 20);
    d->u = BIT(insn, 21);
    d->p = BIT(insn, 22);
    const u32 ext = BF(insn, 31, 28) == 0xF;  /* the v7 MCR2/MRC2 aliases  */
    if (d->l) d->op = ext ? AOP_MRC2 : AOP_MRC;
    else d->op = ext ? AOP_MCR2 : AOP_MCR;
    return d->op;
  }
  if (b27_24 == 0b1100) return AOP_UNDEF;   /* CDP: the 926 has no FPU/MVE */
  if (b27_24 == 0b1101) {
    d->p = BIT(insn, 24);
    d->u = BIT(insn, 23);
    d->w = BIT(insn, 21);
    d->l = BIT(insn, 20);
    d->cp = BF(insn, 11, 8);
    d->crn = BF(insn, 19, 16);
    d->rd = BF(insn, 15, 12);
    d->offset8 = BF(insn, 7, 0) * 4;
    d->am = !d->p ? AM_IMM_POST : (d->w ? AM_IMM_PRE_WB : AM_IMM_PRE);
    d->op = d->l ? AOP_LDC : AOP_STC;
    return d->op;
  }

  return AOP_UNDEF;
}

#undef BF
#undef BIT

}  // namespace brain::arm::isa::armv5
