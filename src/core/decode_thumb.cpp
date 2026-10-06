/* Thumb state decoder -- see core/decode_thumb.h for the contract.
 *
 * Format numbers follow ARM DDI 0100I chapter A6, and the switch is on
 * bits[15:11] (each case therefore covers a 0x800-wide slice of the space):
 *
 *   0  LSL #imm5        8  ALU two low regs (bit10=0) / special (bit10=1)
 *   1  LSR #imm5        9  LDR literal (bit10=0) / unallocated
 *   2  ASR #imm5       10,11  load/store with register offset
 *   3  ADD/SUB reg|imm3 12..15  LDR/STR/LDRB/STRB #imm5
 *   4  MOV #imm8       16,17  STRH/LDRH #imm5
 *   5  CMP #imm8       18,19  STR/LDR [SP,#imm8]
 *   6  ADD #imm8       20,21  ADD Rd,PC|SP,#imm8
 *   7  SUB #imm8       22  ADD/SUB SP | PUSH   23  POP | BKPT
 *   24,25  STMIA/LDMIA  26,27  B<cond> | SVC   28  B
 *   29  BLX-to-ARM (2nd half)  30  long-branch setup  31  BL (2nd half)
 *
 * Two ARMv5T-specific points, because later manuals disagree here:
 *   - format 1 with imm5 == 0 is `MOV Rd, Rm` (CPY) and does NOT update the
 *     flags; from ARMv6 the same encoding is LSLS #0, which does.
 *   - the long branch is a pair of *independent halfword* instructions
 *     (A6.3.2 / A7.1.17: H == 10 sets LR, H == 11 branches, H == 01 branches
 *     to ARM), not one 32-bit instruction as in ARMv6T2.  Each half is
 *     decoded and executed on its own, so no look-ahead is needed and an
 *     isolated halfword behaves the way the manual says it does.
 */
#include <cstring>

#include "core/decode_thumb.h"
#include "core/ops.h"

namespace brain {

namespace {

ALWAYS_INLINE u32 bits(u32 x, u32 hi, u32 lo) {
  return (x >> lo) & ((1u << (hi - lo + 1)) - 1);
}
ALWAYS_INLINE s32 sext(u32 x, u32 n) {
  return (s32)(x << (32 - n)) >> (32 - n);
}

ALWAYS_INLINE void mk_dp(ArmDec* d, u32 dp, u32 s, u32 rd, u32 rn) {
  d->op = AOP_DP;
  d->dp = dp;
  d->s = s;
  d->rd = rd;
  d->rn = rn;
}

/* LDR/STR/LDRB/STRB, offset already computed. */
ALWAYS_INLINE void mk_ldst(ArmDec* d, u32 l, u32 b, u32 rd, u32 rn) {
  d->op = AOP_LDRST;
  d->l = l;
  d->b = b;
  d->rd = rd;
  d->rn = rn;
  d->p = 1;
  d->u = 1;
}

/* LDRH/STRH/LDRSB/LDRSH.  `h` selects halfword (2 bytes) vs byte (1). */
ALWAYS_INLINE void mk_ldsth(ArmDec* d, u32 l, u32 h, u32 sgn, u32 rd, u32 rn) {
  d->op = AOP_LDRSTH;
  d->l = l;
  d->h = h;
  d->signed_ = sgn;
  d->rd = rd;
  d->rn = rn;
  d->p = 1;
  d->u = 1;
}

/* ---- 0x4400-0x47FF: special data processing and branch exchange ---- */
void decode_special(u32 hw, ArmDec* d) {
  const u32 rd = hw & 7u;              /* formats 0-3, 10-17        */
  const u32 op = bits(hw, 9, 8);
  const u32 rm4 = bits(hw, 6, 3);          /* 4-bit Rm: bit6 = Rm[3]         */

  if (op != 3) {
    /* ADD / CMP / MOV between low and high registers.  There is no S bit:
     * only CMP updates the flags (A6.4.2). */
    const u32 rdn = ((hw >> 4) & 8u) | rd;
    switch (op) {
      case 0:                              /* ADD Rdn, Rm                      */
        if (rdn == 15) {                   /* "MOV PC,Rm" via the ADD form     */
          d->op = AOP_BX;
          d->rm = rm4;
          d->kind = 0;
          return;
        }
        mk_dp(d, DP_ADD, 0, rdn, rdn);
        d->op2 = OP2_REG;
        d->rm = rm4;
        return;
      case 1:                              /* CMP Rdn, Rm                      */
        mk_dp(d, DP_CMP, 1, 0, rdn);
        d->op2 = OP2_REG;
        d->rm = rm4;
        return;
      case 2: {                            /* MOV Rd, Rm                       */
        const u32 rdst = ((hw >> 4) & 8u) | rd;
        if (rdst == 15) {
          d->op = AOP_BX;
          d->rm = rm4;
          d->kind = 0;
          return;
        }
        mk_dp(d, DP_MOV, 0, rdst, 0);
        d->op2 = OP2_REG;
        d->rm = rm4;
        return;
      }
      default: break;
    }
    /* op == 3 never gets here (handled below); anything else is unallocated. */
    d->op = AOP_UNDEF;
    return;
  }

  /* 0x47xx: BX (L=0), BLX Rm (L=1), BXJ, BCO -- A6.6.3 / A7.1.19 */
  const u32 L = (hw >> 7) & 1u;
  const u32 low = hw & 7u;
  if (low == 0) {
    d->op = AOP_BX;
    d->rm = rm4;
    d->kind = L ? 1u : 0u;
    return;
  }
  if (!L && low == 2) {                    /* BXJ: enter Jazelle state        */
    d->op = AOP_BX;
    d->rm = rm4;
    d->kind = 2;
    return;
  }
  d->op = AOP_UNDEF;                       /* BCO is not modelled (no EE11)   */
}

}  // namespace

u32 decode_thumb(u32 hw, ArmDec* d) {
  std::memset(d, 0, sizeof(*d));
  d->insn = hw;
  d->cond = 0xF;                            /* unconditional unless B<cond>   */

  const u32 rd = hw & 7u;              /* formats 0-3, 10-17        */
  const u32 rd8 = bits(hw, 10, 8);       /* formats 4-9, 18-21          */
  const u32 imm8 = bits(hw, 7, 0);
  const u32 rn3 = bits(hw, 5, 3);
  const u32 rm3 = bits(hw, 8, 6);
  const u32 imm5 = bits(hw, 10, 6);

  switch (hw >> 11) {

  /* ---- formats 1-2: shift/move by immediate, ADD/SUB ------------------ *
   * 000 type(2) imm5(5) Rm(3) Rdn(3)   and   00011 op(2) Rm|imm3 Rn Rd      */
  case 0: case 1: case 2: {                 /* LSL / LSR / ASR #imm5         */
    const u32 type = (hw >> 11) & 3u;
    if (type == 0 && imm5 == 0) {
      /* MOV Rdn, Rm (CPY): no flag change.  With Rdn = PC it is a BX. */
      if (rd == 15) {
        d->op = AOP_BX;
        d->rm = rm3;
        d->kind = 0;
        return 0;
      }
      mk_dp(d, DP_MOV, 0, rd, 0);
      d->op2 = OP2_REG;
      d->rm = bits(hw, 5, 3);
      return 0;
    }
    if (rd == 15) { d->op = AOP_UNDEF; return 0; }          /* UNPREDICTABLE  */
    mk_dp(d, DP_MOV, 1, rd, rd);
    d->op2 = OP2_REG_SHIFT;
    d->rm = bits(hw, 5, 3);
    d->sh = type;                                /* 0 LSL, 1 LSR, 2 ASR       */
    d->amt = imm5 ? imm5 : 32;                   /* LSR/ASR #0 means #32      */
    return 0;
  }

  case 3: {                                        /* ADD/SUB reg or #imm3    */
    const u32 op = bits(hw, 10, 9);
    const u32 f = bits(hw, 8, 6);
    mk_dp(d, (op & 1u) ? DP_SUB : DP_ADD, 1, rd, rn3);
    if (op < 2) { d->op2 = OP2_REG; d->rm = f; }
    else { d->op2 = OP2_IMM; d->imm12 = f; }
    return 0;
  }


  case 4:  /* MOVS Rd, #imm8 */
    mk_dp(d, DP_MOV, 1, rd8, 0);
    d->op2 = OP2_IMM;
    d->imm12 = imm8;
    return 0;
  case 5:  /* CMP Rn, #imm8 */
    mk_dp(d, DP_CMP, 1, 0, rd8);
    d->op2 = OP2_IMM;
    d->imm12 = imm8;
    return 0;
  case 6:  /* ADDS Rd, #imm8 */
    mk_dp(d, DP_ADD, 1, rd8, rd8);
    d->op2 = OP2_IMM;
    d->imm12 = imm8;
    return 0;
  case 7:  /* SUBS Rd, #imm8 */
    mk_dp(d, DP_SUB, 1, rd8, rd8);
    d->op2 = OP2_IMM;
    d->imm12 = imm8;
    return 0;

  case 8: {
    if (hw & (1u << 10)) { decode_special(hw, d); return 0; }
    /* ALU on two low registers: 0100000 op(4)... with op at bits[9:6]. */
    const u32 op = bits(hw, 9, 6);
    const u32 rm = bits(hw, 5, 3);
    switch (op) {
      case 0x0: mk_dp(d, DP_AND, 1, rd, rd); d->op2 = OP2_REG; d->rm = rm; break;
      case 0x1: mk_dp(d, DP_EOR, 1, rd, rd); d->op2 = OP2_REG; d->rm = rm; break;
      case 0x2: case 0x3: case 0x4: case 0x7: {
        /* LSL/LSR/ASR/ROR by register: `MOV Rdn, Rdn, shift Rs`. */
        mk_dp(d, DP_MOV, 1, rd, rd);
        d->op2 = OP2_REG_RS;
        d->rm = rd;
        d->rs = rm;
        d->sh = (op == 7) ? SH_ROR : op;
        break;
      }
      case 0x5: mk_dp(d, DP_ADC, 1, rd, rd); d->op2 = OP2_REG; d->rm = rm; break;
      case 0x6: mk_dp(d, DP_SBC, 1, rd, rd); d->op2 = OP2_REG; d->rm = rm; break;
      case 0x8: mk_dp(d, DP_TST, 1, 0, rd);  d->op2 = OP2_REG; d->rm = rm; break;
      case 0x9:                                       /* NEG Rd, Rm */
        mk_dp(d, DP_RSB, 1, rd, rm);
        d->op2 = OP2_IMM;
        d->imm12 = 0;
        break;
      case 0xA: mk_dp(d, DP_CMP, 1, 0, rd);  d->op2 = OP2_REG; d->rm = rm; break;
      case 0xB: mk_dp(d, DP_CMN, 1, 0, rd);  d->op2 = OP2_REG; d->rm = rm; break;
      case 0xC: mk_dp(d, DP_ORR, 1, rd, rd); d->op2 = OP2_REG; d->rm = rm; break;
      case 0xD:                                       /* MULS Rd, Rm */
        d->op = AOP_MUL;
        d->rd = rd;
        d->rm = rd;
        d->rs = rm;
        d->s = 1;
        break;
      case 0xE: mk_dp(d, DP_BIC, 1, rd, rd); d->op2 = OP2_REG; d->rm = rm; break;
      default:  mk_dp(d, DP_MVN, 1, rd, 0);  d->op2 = OP2_REG; d->rm = rm; break;
    }
    return 0;
  }

  case 9: {                                         /* LDR literal */
    /* A6.2.24 / ARM926EJ-S A6.3.63: 01001 Rt imm8, and the *only* reserved case
     * is the whole-group one above.  Bit 10 is Rt[2], not an "unallocated" flag,
     * so testing it here made every LDR to R4-R7 from the literal pool undefined -
     * which no case in the corpus could show, since its pool loads all use R0-R3.
     */
    mk_ldst(d, 1, 0, rd8, 15);
    d->am = AM_IMM_PRE;
    d->op2 = OP2_NONE;
    d->imm12 = imm8 << 2;
    return 0;
  }

  case 10: case 11: {                                /* reg-offset loads */
    /* bits[11:9]: 0 STR 1 STRH 2 STRB 3 LDRSB 4 LDR 5 LDRH 6 LDRB 7 LDRSH */
    const u32 f = bits(hw, 11, 9);
    const u32 rm = bits(hw, 8, 6);
    switch (f) {
      case 0: mk_ldst(d, 0, 0, rd, rn3); break;
      case 1: mk_ldsth(d, 0, 1, 0, rd, rn3); break;
      case 2: mk_ldst(d, 0, 1, rd, rn3); break;
      case 3: mk_ldsth(d, 1, 0, 1, rd, rn3); break;
      case 4: mk_ldst(d, 1, 0, rd, rn3); break;
      case 5: mk_ldsth(d, 1, 1, 0, rd, rn3); break;
      case 6: mk_ldst(d, 1, 1, rd, rn3); break;
      default: mk_ldsth(d, 1, 1, 1, rd, rn3); break;
    }
    d->am = AM_REG_PRE;
    d->op2 = OP2_REG;                               /* unshifted Ro (A6.5.3)  */
    d->rm = rm;
    return 0;
  }

  case 12: mk_ldst(d, 0, 0, rd, rn3); d->am = AM_IMM_PRE; d->op2 = OP2_NONE;
            d->imm12 = imm5 * 4; return 0;
  case 13: mk_ldst(d, 1, 0, rd, rn3); d->am = AM_IMM_PRE; d->op2 = OP2_NONE;
            d->imm12 = imm5 * 4; return 0;
  case 14: mk_ldst(d, 0, 1, rd, rn3); d->am = AM_IMM_PRE; d->op2 = OP2_NONE;
            d->imm12 = imm5; return 0;
  case 15: mk_ldst(d, 1, 1, rd, rn3); d->am = AM_IMM_PRE; d->op2 = OP2_NONE;
            d->imm12 = imm5; return 0;

  case 16: case 17: {                                /* STRH / LDRH #imm5*2 */
    const u32 L = (hw >> 11) & 1u;
    mk_ldsth(d, L, 1, 0, rd, rn3);
    d->am = AM_IMM_PRE;
    d->op2 = OP2_NONE;
    d->imm12 = imm5 * 2;
    return 0;
  }

  case 18: case 19: {                                /* [SP, #imm8*4] */
    const u32 L = (hw >> 11) & 1u;
    mk_ldst(d, L, 0, bits(hw, 10, 8), 13);
    d->am = AM_IMM_PRE;
    d->op2 = OP2_NONE;
    d->imm12 = imm8 << 2;
    return 0;
  }

  case 20:                                           /* ADD Rd, PC, #imm8*4 */
    mk_dp(d, DP_ADD, 0, rd8, 15);
    d->op2 = OP2_IMM;
    d->imm12 = imm8 << 2;
    return 0;
  case 21:                                           /* ADD Rd, SP, #imm8*4 */
    mk_dp(d, DP_ADD, 0, rd8, 13);
    d->op2 = OP2_IMM;
    d->imm12 = imm8 << 2;
    return 0;

  case 22: {
    /* A6.3.5/A6.3.9: bit[10:9] = 00 is `ADD/SUB SP, #imm7*4` and 10 is PUSH;
     * 01 and 11 (`1011 001x`, `1011 011x`) are ARMv6T2's LDM/STM-without-writeback
     * and are unallocated here.  Testing only bit 10 made both of those execute -
     * `1011 011x` as a PUSH - which is how the BKPT/IT mix-up below went unseen. */
    if ((hw >> 9) == 0b1011000) {                     /* ADD/SUB SP, #imm7*4 */
      const u32 imm7 = bits(hw, 6, 0);
      mk_dp(d, ((hw >> 8) & 1u) ? DP_SUB : DP_ADD, 0, 13, 13);
      d->op2 = OP2_IMM;
      d->imm12 = imm7 << 2;
      return 0;
    }
    if ((hw >> 9) != 0b1011010) {                     /* ARMv6T2 LDM/STM: here */
      d->op = AOP_UNDEF;
      return 0;
    }
    {                                                 /* PUSH {imm8[,LR]}      */
      const u32 M = (hw >> 8) & 1u;
      d->op = AOP_STM;
      d->l = 0;
      d->p = 1;
      d->u = 0;                                        /* STMDB semantics     */
      d->w = 1;
      d->rn = 13;
      d->kind = 0;
      d->list = imm8 | (M ? (1u << 14) : 0u);
      if (d->list == 0) d->op = AOP_UNDEF;             /* empty list          */
      return 0;
    }
  }

  case 23: {
    if ((hw >> 9) == 0b1011110) {                      /* POP {imm8[,PC]}     */
      const u32 M = (hw >> 8) & 1u;
      d->op = AOP_LDM;
      d->l = 1;
      d->p = 0;
      d->u = 1;
      d->w = 1;
      d->rn = 13;
      d->kind = 0;                    /* A7.1.49: POP updates only the T bit  */
      d->list = imm8 | (M ? (1u << 15) : 0u);
      if (d->list == 0) d->op = AOP_UNDEF;
      return 0;
    }
    if ((hw >> 8) == 0xBE) {                           /* BKPT #imm8          */
      d->op = AOP_BKPT;
      d->imm24 = imm8;
      return 0;
    }
    d->op = AOP_UNDEF;               /* v6 NOP/SEV/YIELD, LDM/STM without WB  */
    return 0;
  }

  case 24: case 25: {                                  /* STMIA / LDMIA       */
    /* A6.4.10/A6.4.11: 0b1100 0 L Rn(2) list -- L is bit[11], which is also
     * the bit that splits case 24 (STMIA) from case 25 (LDMIA).  Rn is a
     * 2-bit field: R8-R15 cannot be used as the base here. */
    const u32 L = (hw >> 11) & 1u;
    if (imm8 == 0) { d->op = AOP_UNDEF; return 0; }
    d->op = L ? AOP_LDM : AOP_STM;
    d->l = L;
    d->p = 0;                            /* both STMIA and LDMIA increment    */
    d->u = 1;                            /* after the transfer, base +/- n*4 */
    d->w = 1;
    d->rn = bits(hw, 9, 8);
    d->kind = 0;
    d->list = imm8;
    return 0;
  }

  case 26: case 27: {                                  /* B<cond>, SVC        */
    if ((hw >> 8) == 0xDF) {
      d->op = AOP_SWI;
      d->imm24 = imm8;
      return 0;
    }
    /* cond sits in bits[11:8] (1101 cond:4 imm8), *not* in [9:6]: reading the
     * wrong field still produced the right branch target (the offset is
     * bits[7:0]) but evaluated a different condition, and eight of the fifteen
     * conditions happen to agree with the true one for any flag set, which is
     * why this hid for so long. */
    const u32 cond = bits(hw, 11, 8);
    if (cond >= 0xE) { d->op = AOP_UNDEF; return 0; }  /* NV/AL are v6+       */
    d->op = AOP_B;
    d->kind = 3;
    d->cond = cond;
    d->imm24 = (u32)(sext(imm8, 8) << 1);
    return 0;
  }

  case 28:                                             /* B #imm11*2 */
    d->op = AOP_B;
    d->kind = 0;
    d->imm24 = (u32)(sext(bits(hw, 10, 0), 11) << 1);
    return 0;

  case 29:                                             /* BLX to ARM (2nd) */
    d->op = AOP_B;
    d->kind = 6;
    d->imm24 = (u32)(sext(bits(hw, 10, 0), 11) << 1);
    return 0;

  case 30:                                             /* long-branch setup */
    d->op = AOP_B;
    d->kind = 4;
    d->imm24 = (u32)(sext(bits(hw, 10, 0), 11) << 12);
    return 0;

  case 31:                                             /* BL (2nd half) */
    d->op = AOP_B;
    d->kind = 5;
    d->imm24 = (u32)(sext(bits(hw, 10, 0), 11) << 1);
    return 0;
  }

  d->op = AOP_UNDEF;
  return 0;
}

}  // namespace brain
