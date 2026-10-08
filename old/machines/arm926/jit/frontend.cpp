/* Guest ARM -> JIT IR, and IR -> AArch64.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The translation reads the same decoder the interpreter does, so the two cannot
 * disagree about what an encoding means: `decode_arm` fills an `ArmDec`, and this
 * file only has to decide whether the result is something a block may contain and
 * then re-shape it.  That is the whole reason the IR exists - the ARM side is
 * written once, next to the decoder, and a backend never sees an instruction.
 */
#include "machines/arm926/jit/frontend.h"

#include <cstring>

#include "machines/arm926/cpu/decode.h"
#include "machines/arm926/cpu/decode_thumb.h"
#include "machines/arm926/cpu/ops.h"          /* CycleModel: the cycle charge rides in the IR */
#include "machines/arm926/jit/emit_arm64.h"
#include "machines/arm926/jit/jit.h"      /* jit_fast_mem_ok: may a block access memory?  */

namespace brain::arm926 {

namespace {

/* The ARM data-processing opcodes, as `ArmDec::dp` numbers them. */
enum {
  DP_AND = 0, DP_EOR = 1, DP_SUB = 2, DP_RSB = 3, DP_ADD = 4, DP_ADC = 5,
  DP_SBC = 6, DP_RSC = 7, DP_TST = 8, DP_TEQ = 9, DP_CMP = 10, DP_CMN = 11,
  DP_ORR = 12, DP_MOV = 13, DP_BIC = 14, DP_MVN = 15,
};

/* IR register numbers are guest register numbers (the IR stays independent of the
 * target), and the shadowed state puts R0-R12 in X1-X13 - X0 is the state pointer,
 * so the mapping is off by one and every use of it has to go through here. */
uint32_t reg_of(uint32_t r) { return r + 1u; }

/* One DP instruction -> IR.  Returns false if it is out of the accepted set, in
 * which case the caller ends the block *before* this instruction.
 *
 * `thumb` is the state the word was decoded in: it selects the pc a Thumb
 * pc-relative operand reads (+4, word aligned) and, more importantly, says
 * whether the ARM condition field even exists.  decode_thumb() fills an ArmDec
 * with `cond = 0xF` (its "unconditional" marker) and only ever sets a condition
 * for `B<cond>`, so a Thumb instruction is never predicated and this check is
 * the ARM one. */
/* The operand shapes the carry-consuming arithmetic (ADC/SBC/MVN's neighbours,
 * see translate_dp) can take: one where the *shifter* supplies no carry, so the
 * instruction's carry-in is the C flag.  A register operand has no shift at all
 * (`decode_op2_reg` reports OP2_REG only for `Rm` with no shift bits set, and
 * RRX/ROR #0 as a separate shape), and an immediate with a zero rotate field is
 * the raw imm8.  Everything else is the ARMv5 shifter-carry rule and stays with
 * the dispatcher. */
bool adc_sbc_operand(const ArmDec& d) {
  if (d.op2 == OP2_REG) return true;
  return d.op2 == OP2_IMM && d.rot == 0;
}

bool translate_dp(const ArmDec& d, uint32_t pc, bool thumb, IrInsn* out) {
  /* `ArmDec::cond` is the Thumb branch field; for ARM words the decoder reports a
   * failed condition as AOP_SATISFY and executes nothing, so the only thing left to
   * check here is that the block is not asked to run an instruction the dispatcher
   * would skip: a conditional one would have to be predicated instead. */
  if (!thumb && ((d.insn >> 28) & 0xFu) != 0xEu) return false;    /* conditional: dispatcher's job */

  /* `ADR Rd, #label` (T1, and the SP-relative form of the same group): the
   * Thumb decoder turns it into `ADD Rd, PC/SP, #imm8*4`, and with PC as the
   * base the whole result is a constant of the translation - which is what
   * ADR is for, so it is worth folding rather than ending the block.  The pc a
   * Thumb instruction reads is `insn + 4`, *word aligned* (A6.5.2: bits[1:0]
   * are treated as zero, so a halfword-aligned ADR still yields an aligned
   * address).  The SP form (rn == 13) stays out: R13 lives in the shadow, not
   * in the allocator's window, and ADR to SP is rare enough not to pay for. */
  if (thumb && d.rn == 15 && d.op2 == OP2_IMM) {
    if (d.dp != DP_ADD || d.s) return false;      /* only ADR is a constant */
    IrInsn in{};
    in.op = IR_MOVI;
    in.dst = uint8_t(d.rd);
    /* decode_thumb() leaves an immediate unrotated (the encoding has no rotate
     * field at all), so `imm12` is already the value. */
    in.imm = ((pc + 4u) & ~3u) + d.imm12;
    *out = in;
    return true;
  }
  if (d.op2 == OP2_REG_RS) return false;         /* shift by a register          */
  /* Only R0-R13 are addressable through the ABI's X1-X14 window; SP/LR/PC (and
   * banked registers in general) stay the interpreter's, and the dispatcher is what
   * handles them, so a block ends here instead. */
  if (d.rd > 12u || d.rn > 12u || d.ra > 12u) return false;
  /* `d.rm` is only *a register* when the encoding says so, and `d.rs` only for the
   * register-*specified* shift: decode_arm() pre-seeds `rm` with the instruction's
   * low nibble - which for an immediate is part of the immediate itself (`and r2,
   * r1, #0x0F` would read rm = 15, so a blanket check refuses every value ending in
   * 0xD..0xF) - and for an immediate shift it holds the shift amount in bits[11:7],
   * so a check on `rs` there refuses every shift by 13..31.  Both forms of the
   * mistake turn a working encoding into a silent fallback to the interpreter,
   * which is why they are named per shape here rather than lumped together. */
  if ((d.op2 == OP2_REG || d.op2 == OP2_REG_SHIFT) && d.rm > 12u) return false;
  if (d.op2 == OP2_REG_RS && (d.rm > 12u || d.rs > 12u)) return false;
  if (d.op2 != OP2_NONE && d.op2 != OP2_IMM && d.op2 != OP2_REG &&
      d.op2 != OP2_REG_SHIFT)
    return false;

  /* Flag-setting forms with an immediate operand, and how the C they produce is
   * recovered on each host.
   *
   * *Arithmetic* (ADD/SUB/RSB/RSC/CMN/CMP): the ARM C bit comes out of the adder
   *   wherever the second operand came from, which is what the host computes too,
   *   so there is nothing to recover.  This is the common case in real code -
   *   `adds r0, r0, #1` is the single most frequent flag-setting ARM word there
   *   is, and `cmp`/`adds` are the whole of a counted loop.
   *
   * *Logical* (AND/EOR/ORR/BIC/MOV/MVN/TST/TEQ): C is the last bit the rotator
   *   shifted out, and *unchanged* when the rotate field is zero (which is every
   *   Thumb immediate, since that encoding has no rotate field).  V is never
   *   written at all.  Both hosts get this wrong on their own - every `and`/`orr`
   *   clears the host's C, and AArch64's flag-setting forms write all four flags -
   *   so both emitters repair it instead: the rotation is known here, so `imm_rot`
   *   tells a backend whether to keep C (a snapshot of the live flags, taken
   *   before the operation) or to substitute the rotator bit, which for an
   *   immediate is a constant (bit 31 of the value materialised below).
   *
   * *Shifts by a register* are a different matter and still end the block: the
   *   amount is a runtime value rather than part of the encoding, and the flags it
   *   produces are the last bit shifted out in C with everything else - V included -
   *   exactly as it was (the interpreter and the model agree on that, including the
   *   entry where only V is set).  `MOVS Rd, Rm, LSL Rs` is the shape: rare enough
   *   not to be worth an instruction each. */

  IrInsn in{};
  in.dst = uint8_t(d.rd);
  in.a = uint8_t(d.rn);
  in.b = uint8_t(d.rm);
  in.setflags = d.s != 0;
  in.b_is_imm = d.op2 == OP2_IMM;
  /* `ArmDec::imm12` is the raw 12-bit field: the value operand2 takes is that field
   * rotated right by twice the rotate field, exactly as the interpreter computes it
   * from the same two pieces. */
  in.imm = d.op2 == OP2_IMM
                ? ((d.imm12 << ((32u - 2u * d.rot) & 31u)) |
                   (d.imm12 >> ((2u * d.rot) & 31u)))
                : 0u;
  if (d.op2 == OP2_IMM && d.rot == 0) in.imm = d.imm12;
  /* The rotate field is kept: a backend that can express the rotator's carry
   * (x86-64 can, as a constant) needs to know whether there *was* one. */
  in.imm_rot = (d.op2 == OP2_IMM) ? (uint8_t)(d.rot & 0xF) : 0;
  in.shift_type = 0;
  in.shift_amt = 0;

  /* The barrel shifter, which AArch64 has as an operand field rather than as an
   * instruction: 00 = LSL, 01 = LSR, 10 = ASR, 11 = ROR.  A zero amount does not
   * mean "no shift" for three of those four (ARM's encodings are 1..32 and RRX),
   * which is why the cases below are spelled out rather than passed through. */
  bool rrx = false;
  if (d.op2 == OP2_REG_SHIFT) {
    if (d.sh > SH_RRX) return false;
    in.shift_type = uint8_t(d.sh);
    in.shift_amt = d.amt;
    if (d.sh == SH_RRX) {
      rrx = true;                      /* RRX: rotate right *through the carry* */
      in.shift_amt = 0;
    } else if (d.sh == SH_LSL) {
      if (d.amt > 31) return false;    /* no-op LSL #0 is legal, LSL #32 is not */
    } else if (d.amt == 0 || d.amt > 31) {
      return false;      /* a 32-bit shift (what `LSR #0`/`ASR #0` mean) or worse:
                          * AArch64's shift field stops at 31, so these stay with the
                          * dispatcher.  A zero amount cannot come out of
                          * decode_op2_reg(); refusing it keeps a hand-built IrInsn
                          * from being read as "no shift" here and "32" elsewhere. */
    }
    /* A shift by a register (RRX with S, or any of the four) also has to leave V
     * alone, so it stays with the dispatcher: see the C/V repair below. */
    if (d.op2 == OP2_REG_RS) return false;
  } else if (d.op2 == OP2_REG) {
    in.shift_amt = 0;
  }

  switch (d.dp) {
    case DP_AND: in.op = IR_AND; break;
    case DP_EOR: in.op = IR_EOR; break;
    case DP_SUB: in.op = IR_SUB; break;
    case DP_RSB: in.op = IR_RSB; break;
    case DP_ADD: in.op = IR_ADD; break;
    /* ADC/SBC read C as a *value* and write it from the sum, which both hosts
     * compute natively (`adc`/`adcs`, `sbb`/`sbcs`).  What they cannot do is the
     * operand shape ARMv5 feeds them: when the shifter itself produces a carry
     * (a register-specified shift with a non-zero amount, or a rotated
     * immediate), that carry is what ADC/SBC/RSC take *in* - instead of C - and
     * no host instruction has a second carry input.  Those shapes end the block;
     * what is left is the plain register operand and the unrotated immediate,
     * where the carry-in is the C flag and the carry-out is the sum's, on ARM
     * and on both hosts. */
    case DP_ADC:
      if (!adc_sbc_operand(d)) return false;
      in.op = IR_ADC;
      break;
    case DP_SBC:
      if (!adc_sbc_operand(d)) return false;
      in.op = IR_SBC;
      break;
    /* RSC is the same operation with the shifter operand as the minuend
     * (`Rd = Op2 - Rn - NOT(C)`, A4.1.61), so it takes any operand shape RSB
     * takes - and, like RSB, is *not* limited to the flag-free forms: C and V
     * come out of the sum on AArch64 exactly as ARM defines them. */
    case DP_RSC: in.op = IR_RSC; break;
    case DP_ORR: in.op = IR_ORR; break;
    case DP_BIC: in.op = IR_BIC; break;
    case DP_MOV:
      in.op = d.op2 == OP2_IMM ? IR_MOVI : IR_MOV;
      if (d.op2 == OP2_REG_SHIFT) {
        /* `MOV Rd, Rm, <shift>` is a shift, not a move: name the op after the
         * shift so the backend does not have to look anywhere else. */
        static const IrOp kShift[] = {IR_LSL, IR_LSR, IR_ASR, IR_ROR};
        in.op = rrx ? IR_RRX : kShift[d.sh & 3];
      }
      in.a = in.b;                                   /* the source, not Rn */
      in.b = 0;
      break;
    case DP_MVN:
      in.op = IR_MVN;
      in.a = in.b;
      in.b = 0;
      break;
    /* The flag-only forms keep the operation and drop the destination, which is
     * how AArch64 spells them too: `ADDS WZR, ...` is CMP, `ANDS WZR, ...` is TST. */
    case DP_TST: in.op = IR_AND; in.dst = IR_NO_DST; in.setflags = true; break;
    case DP_TEQ: in.op = IR_EOR; in.dst = IR_NO_DST; in.setflags = true; break;
    case DP_CMP: in.op = IR_SUB; in.dst = IR_NO_DST; in.setflags = true; break;
    case DP_CMN: in.op = IR_ADD; in.dst = IR_NO_DST; in.setflags = true; break;
    default: return false;
  }
  /* A MOV of a shifted register is a shift of one operand, not a two-operand op. */
  /* MVN with a shifted register is fine: `ORN Wd, WZR, Rm, <sh>` is the value and
   * `TST Wd, Wd` the flags, with C and V repaired as for every other logical. */
  *out = in;
  return true;
}

/* The addressing mode of a load/store as the IR spells it: `post` is what the
 * access reads, `wb` whether the base is written afterwards. */
bool wb_of(const ArmDec& d, uint8_t* post, uint8_t* wb) {
  switch (d.am) {
    case AM_IMM_PRE: case AM_REG_PRE:
      *post = 0; *wb = MEM_WB_NONE; return true;
    case AM_IMM_PRE_WB: case AM_REG_PRE_WB:
      *post = 0; *wb = MEM_WB_YES; return true;
    case AM_IMM_POST: case AM_REG_POST:
      *post = 1; *wb = MEM_WB_YES; return true;
    default: return false;                 /* AM_NONE / AM_NONE_ADDR: not here */
  }
}

/* One guest access -> IR, or false if a block may not perform it.  The verdict
 * is taken once per *block*, not once per access: `jit_fast_mem_ok` is the same
 * test the dispatcher puts in the shadow for the interpreter's fast path (MMU
 * off, identity RAM window, no alignment check, no BE8 lane swap), so a
 * translated access does exactly what that path does - and the emitter guards
 * every access with the window anyway, leaving the block if the *address* turns
 * out not to be RAM.
 *
 * Refused, each because a block cannot reproduce it:
 *   - R13/R14/R15 as the loaded or stored register (the dispatcher owns the
 *     pipeline and the banked registers);
 *   - a load whose written-back base is also its destination (ARM keeps the
 *     loaded value in it, which one register cannot hold both of);
 *   - a shift amount supplied by a register, LDRD/STRD, LDM/STM, SWP;
 *   - a writeback to R15.
 * A pc-relative address *is* translated: the pipeline value is a constant of
 * the translation, so the address is one too (MEM_ABS).
 */
bool translate_ldst(const Cpu& cpu, uint32_t pc, const ArmDec& d, bool extra, bool thumb,
                    IrInsn* out) {
  if (!jit_fast_mem_ok(&cpu)) return false;
  /* A conditional instruction executes or does not, and a *block* has no way
   * to skip one: predication belongs to the dispatcher (which is also where
   * the branch counters see it).  translate_dp() makes the same test, and
   * Thumb has no predicated loads or stores at all. */
  if (!thumb && ((d.insn >> 28) & 0xFu) != 0xEu) return false;
  if (d.rd > 12) return false;
  const bool load = d.l != 0;
  if (load && d.w && d.rd == d.rn) return false;

  IrInsn in{};
  in.op = load ? IR_LD : IR_ST;
  in.dst = uint8_t(d.rd);
  in.mem_signed = 0;
  if (extra) {
    /* LDRH/STRH/LDRSB/LDRSH: H selects 16 bits, S sign-extends, and a store in
     * this space is the halfword form whatever H says (A5.3.3). */
    if (!load) {
      in.mem_size = 2;
    } else {
      /* bit5 (H) is the width, bit6 (S) the sign: LDRH 10x1, LDRSB 1x01 and
       * LDRSH 11x1 - so a *signed* halfword load is 16 bits that sign-extend,
       * which is the one shape the width alone gets wrong. */
      in.mem_size = d.h ? 2 : 1;
      in.mem_signed = d.signed_ ? 1 : 0;
    }
  } else {
    in.mem_size = d.b ? 1 : 4;
  }
  in.mem_sub = d.u ? 0 : 1;

  /* ---- the base ---- */
  if (d.rn == 15) {
    /* pc-relative: the instruction reads pc + 8, and in the plain offset form
     * the whole address is a constant of the translation.  Anything that would
     * write back to R15 is refused: the interpreter writes the *pipeline*
     * register there (the next fetch still comes from npc), while a block's
     * R15 output is the address execution resumes at - the two cannot be told
     * apart from inside the block.  Note P=0 writes back even with W=0 (the
     * ARM926EJ-S keeps the base result; Unicorn agrees), so `am` is the test. */
    if (d.am != AM_IMM_PRE) return false;
    in.mem_reg = MEM_ABS;
    in.mem_shadow = 0;
    in.a = 0;
    /* The pc an instruction reads depends on the state (ARM: +8 and word
     * aligned by construction; Thumb: +4 with bits[1:0] treated as zero, which
     * is what makes a halfword-aligned literal load reach its word). */
    const uint32_t base = thumb ? ((pc + 4u) & ~3u) : (pc + 8u);
    in.imm = base + (d.u ? d.imm12 : (uint32_t)(-(int32_t)d.imm12));
    *out = in;
    return true;
  }
  if (d.rn > 14) return false;
  in.a = uint8_t(d.rn);
  in.mem_shadow = d.rn >= 13 ? 1 : 0;      /* R13/R14 live in the shadow */
  if (!wb_of(d, &in.mem_post, &in.mem_wb)) return false;
  /* A load whose written-back base is also its destination: the interpreter
   * skips the writeback when `rd == rn` (interp_arm.cpp), and that is what
   * makes `ldr r0, [r0, #4]!` leave the loaded value in r0.  A block gets the
   * same result for free by dropping the writeback - the load already targets
   * the base - while keeping the addressing form. */
  if (load && d.rd == d.rn) in.mem_wb = MEM_WB_NONE;

  if (d.op2 == OP2_REG_RS) return false;   /* shift amount in a register  */
  if (d.op2 == OP2_REG || d.op2 == OP2_REG_SHIFT) {
    /* [Rn, +-Rm] or [Rn, +-Rm, <shift> #imm].  The shift rules are the same as
     * for a data-processing operand: LSL #0 is no shift at all, and the
     * encodings that mean a 32-bit shift have no host instruction behind them
     * in a five-bit field. */
    if (d.rm > 12) return false;
    in.mem_reg = MEM_REG;
    in.b = uint8_t(d.rm);
    if (d.op2 == OP2_REG_SHIFT) {
      if (d.sh == SH_RRX) return false;
      if (d.sh == SH_LSL) {
        if (d.amt > 31) return false;
      } else if (d.amt == 0 || d.amt > 31) {
        return false;
      }
      in.shift_type = uint8_t(d.sh);
      in.shift_amt = uint8_t(d.amt);
    }
  } else {
    in.mem_reg = MEM_IMM;
    in.imm = d.imm12;
  }
  *out = in;
  return true;
}

/* One branch -> IR.  A branch ends a block (see ir.h): the emitter evaluates
 * the condition at run time and chooses between the target and the
 * fall-through, so a conditional branch is translated as well as an
 * unconditional one and the block containing it performs the branch instead of
 * handing it back to the interpreter.
 *
 * `imm` is the taken target (a direct branch) and `link` the value of a BL/BLX
 * link - written by the taken path only, because a branch whose condition is
 * false writes neither the PC nor LR.  Where a *straight-line* exit goes is the
 * block's `next_pc`: the target for an unconditional branch, the fall-through
 * for a conditional one.
 */
bool translate_branch(const Cpu* cpu, uint32_t pc, const ArmDec& d, uint32_t op,
                      bool thumb, IrBlock* out, IrInsn* ir) {
  IrInsn in{};
  in.op = IR_BR;
  in.dst = IR_NO_DST;
  in.cond = 0xEu;
  in.br_link = 0;
  in.br_reg = 0;
  in.br_mask = BR_MASK_STATE;

  if (thumb) {
    /* ---- Thumb state.  The pc an instruction reads is +4, a branch target is
     * only halfword aligned, and the *exit address carries the state*: bit 0 of
     * whatever a block leaves in R15 is what the dispatcher switches on, and
     * every branch here either stays in Thumb (bit 0 set) or is a BLX that
     * leaves for ARM (masked to a word).  The one predicated instruction in the
     * whole state is `B<cond>`, whose condition comes from the encoding's own
     * field rather than from insn[31:28]. */
    const uint32_t next = (pc + 2u) | 1u;              /* stay in Thumb */
    if (op == AOP_B && d.kind == 3) {                  /* B<cond> #imm8 */
      in.cond = uint8_t(d.cond);
      in.imm = ((pc + 4u) + d.imm24) | 1u;
      in.br_mask = BR_MASK_HALF;
      out->next_pc = next;                             /* fall-through: pc+2 */
      *ir = in;
      return true;
    }
    if (op == AOP_B && d.kind == 0) {                  /* B #imm11 */
      in.imm = ((pc + 4u) + d.imm24) | 1u;
      out->next_pc = in.imm;                           /* unconditional */
      *ir = in;
      return true;
    }
    if (op == AOP_B && (d.kind == 5 || d.kind == 6)) {
      /* The second half of the long-branch pair (A7.1.17): the branch target is
       * *LR + imm24*, not pc-relative - that is what lets an assembler place the
       * pair anywhere in a ±4MB window - and the link replaces LR with the
       * address of the next halfword, T bit set.  kind 6 is the `BLX` half,
       * which leaves for ARM state, so its target drops bits[1:0]; kind 5 stays
       * in Thumb and drops bit 0 only.  (The *first* half, kind 4, only computes
       * LR and is refused: it writes R14 as a data value, which is not something
       * the register window a block works in can hold - see jit/ir.h.) */
      in.br_reg = 1;
      in.b = 14;
      in.imm = d.imm24;
      in.br_mask = (d.kind == 6) ? BR_MASK_WORD : BR_MASK_HALF;
      in.br_link = 1;
      in.link = next;
      out->next_pc = next;
      *ir = in;
      return true;
    }
    if (op == AOP_BX) {
      /* BX Rm (kind 0) and BLX Rm (kind 1) are the same operation with and
       * without a link, and the target register decides the state: bit 0 goes
       * to the dispatcher untouched, exactly as in ARM state.  BXJ (kind 2) is
       * an undefined instruction here, and the Jazelle trap a `bx` can take is
       * reported by the interpreter, so both stay with it. */
      if (d.kind > 1) return false;
      if (d.rm > 14) return false;
      if (cpu && cpu->cfg.jazelle_trap) return false;
      in.br_reg = 1;
      in.b = uint8_t(d.rm);
      if (d.kind == 1) { in.br_link = 1; in.link = next; }
      out->next_pc = next;
      *ir = in;
      return true;
    }
    return false;                                  /* LDM/STM/SWI/undefined */
  }

  if (op == AOP_B) {
    if (d.kind >= 4) return false;             /* Thumb long-branch halves  */
    const uint32_t cond = (d.insn >> 28) & 0xFu;
    if (d.kind == 2) {
      /* BLX #imm: unconditional (its condition field is the fixed 1111), links
       * to pc + 4, and switches to Thumb.  The dispatcher reads the state out
       * of bit 0 of the exit address, so the target carries it. */
      in.imm = (((pc + 8u) + d.imm24) & ~1u) | 1u;
      in.br_link = 1;
      in.link = pc + 4u;
      out->next_pc = in.imm;
      *ir = in;
      return true;
    }
    if (cond == 0xFu) return false;            /* NV: not a branch we run   */
    in.cond = uint8_t(cond);
    in.imm = (pc + 8u) + d.imm24;
    if (d.kind == 1) { in.br_link = 1; in.link = pc + 4u; }
    /* The interpreter applies `& ~3` to an ARM-state branch target; the
     * dispatcher does the same to whatever the block leaves in the exit word,
     * so the raw target is the right thing to put there. */
    out->next_pc = cond == 0xEu ? in.imm : pc + 4u;
    *ir = in;
    return true;
  }
  /* AOP_BX: BX / BLX Rm in ARM state.  The target is whatever the register
   * holds - including the ARM/Thumb bit the dispatcher decodes - and BLX also
   * links.  BXJ is an undefined instruction and stays with the dispatcher, as
   * does the Jazelle trap a `bx` can take. */
  if (d.kind == 2) return false;
  if (d.rm > 14) return false;
  if (d.kind == 0 && cpu && cpu->cfg.jazelle_trap) return false;
  const uint32_t cond = (d.insn >> 28) & 0xFu;
  if (cond == 0xFu) return false;
  in.cond = uint8_t(cond);
  in.br_reg = 1;
  in.b = uint8_t(d.rm);
  if (d.kind == 1) { in.br_link = 1; in.link = pc + 4u; }
  out->next_pc = pc + 4u;
  *ir = in;
  return true;
}

/* One instruction word as the *fetch* would see it: identity-mapped RAM is read
 * straight out of the array, anything else has to walk the TLB the way
 * `Cpu::prepare_fetch()` does, and a translation that does not resolve simply
 * ends the block (the dispatcher runs the instruction, and takes its abort if
 * that is what the fetch does).  Reading physical memory at `pc` instead would
 * happily translate bytes the guest never sees.  If `prepare_fetch()` changes,
 * this has to change with it. */
bool fetch_word(Cpu& cpu, uint32_t va, bool thumb, uint32_t* out) {
  const uint32_t n = thumb ? 2u : 4u;
  if (cpu.ram_fast && ((va - cpu.ram_gbase) & ~cpu.ram_mask) == 0) {
    std::memcpy(out, cpu.ram + (va & cpu.ram_mask), n);   /* VA == PA here */
    if (thumb) *out &= 0xFFFFu;
    return true;
  }
  TlbEntry* e = cpu.tlb_lookup(va & ~0xFFFu, MF_FETCH | (thumb ? MF_SZ16 : MF_SZ32));
  if (!e) return false;
  *out = cpu.phys_read(e->gpa + (va & 0xFFFu), n) & (thumb ? 0xFFFFu : ~0u);
  return true;
}

/* `words != nullptr` translates a caller-supplied array (a test's, or the
 * `jit_emit` tool's); otherwise the words are fetched from `cpu` one at a time,
 * so a block that ends at its first instruction costs one fetch and not a whole
 * window of them.  Without a machine there is no verdict about memory (see
 * `translate_ldst`), so a caller-supplied array translates the register and
 * control-flow subset only; branches do not need a machine. */
uint32_t translate(Cpu* cpu, const uint32_t* words, uint32_t n, uint32_t pc,
                   bool thumb, IrBlock* out) {
  /* The address step: 2 bytes in Thumb state, 4 in ARM.  Everything below that
   * turns an instruction index into a guest address uses it, which is why a
   * Thumb block's `code_end` and the resume address of its bails are halfword
   * addresses (and why the *exit* address carries the T bit - see the branch
   * translation and jit_cache_enter). */
  const uint32_t step = thumb ? 2u : 4u;
  out->n = 0;
  out->start_pc = pc;
  out->next_pc = pc;
  out->code_end = pc;
  out->guest_insns = 0;
  out->dirty = 0;
  out->cycles = 0;
  out->uses_mem = false;
  out->mem_gbase = 0;
  out->mem_mask = 0;
  out->truncated = false;
  out->thumb = thumb;
  out->stop = "empty";
  uint32_t i = 0;
  for (; i < n; ++i) {
    const uint32_t here = pc + i * step;
    uint32_t word;
    if (words) {
      word = thumb ? (words[i] & 0xFFFFu) : words[i];
    } else if (!fetch_word(*cpu, here, thumb, &word)) {
      out->stop = i ? "fetch" : "unsupported";
      break;
    }
    ArmDec d{};
    /* The two decoders have different return conventions: `decode_arm` reports
     * the operation it found, while `decode_thumb` (like the interpreter's own
     * call site) always returns 0 and puts the operation in the record. */
    u32 op;
    if (thumb) {
      decode_thumb(word & 0xFFFFu, &d);
      op = d.op;
    } else {
      op = decode_arm(word, &d);
    }
    IrInsn in{};
    uint32_t cyc = CycleModel::kIssue;
    bool ok = false;
    bool branch = false;
    switch (op) {
      case AOP_DP:
        ok = translate_dp(d, here, thumb, &in);
        break;
      case AOP_LDRST:
      case AOP_LDRSTH:
        if (!cpu) break;                     /* no machine, no memory verdict */
        ok = translate_ldst(*cpu, here, d, op == AOP_LDRSTH, thumb, &in);
        if (ok) cyc += CycleModel::kAccess;
        break;
      case AOP_B:
      case AOP_BX:
        ok = translate_branch(cpu, here, d, op, thumb, out, &in);
        if (ok) {
          branch = true;
          /* A taken branch restarts the prefetch stage; a conditional one that
           * is not taken is just an issue slot, which `cyc_nt` carries so the
           * two exits of the block charge what the interpreter would. */
          cyc += CycleModel::kBranch;
          if (op == AOP_B && d.kind == 2) cyc += CycleModel::kPcLoad;
          in.cyc_nt = (uint16_t)(out->cycles + CycleModel::kIssue);
        }
        break;
      default:
        out->stop = i ? "not translated" : "unsupported";
        break;
    }
    if (op == AOP_DP || op == AOP_LDRST || op == AOP_LDRSTH) {
      if (!ok) {
        out->stop = i ? "outside the translated subset" : "unsupported";
        break;
      }
    } else if (!ok) {
      break;                                 /* the switch set the reason */
    }
    if (in.dst != IR_NO_DST && in.op != IR_ST) out->dirty |= 1u << in.dst;
    if (in.op == IR_LD || in.op == IR_ST) {
      out->uses_mem = true;
      out->mem_gbase = cpu->ram_gbase;
      out->mem_mask = cpu->ram_mask;
    }
    out->cycles += cyc;
    in.cyc = (uint16_t)out->cycles;
    if (!out->push(in)) {
      out->stop = "block too long";
      ++i;
      break;
    }
    out->guest_insns++;
    out->code_end = here + step;
    /* A straight-line exit resumes in the state the block is *in*, and the exit
     * address is what carries that: a Thumb block's fall-through address gets
     * bit 0, exactly like its branch targets, or the dispatcher would read the
     * result as an ARM address and clear CPSR.T. */
    if (!branch) out->next_pc = thumb ? (out->code_end | 1u) : out->code_end;
    out->stop = branch ? "branch" : "budget";
    if (branch) { ++i; break; }              /* a branch is the way out */
  }
  return i;
}

}  // namespace

uint32_t jit_translate_arm_words(const uint32_t* words, uint32_t n, uint32_t pc,
                                 IrBlock* out) {
  return translate(nullptr, words, n, pc, /*thumb=*/false, out);
}

uint32_t jit_translate_arm_block(Cpu& cpu, uint32_t pc, uint32_t max_words,
                                 IrBlock* out) {
  const uint32_t n = max_words < uint32_t(IR_MAX_INSNS) ? max_words
                                                       : uint32_t(IR_MAX_INSNS);
  return translate(&cpu, nullptr, n, pc, /*thumb=*/false, out);
}

uint32_t jit_translate_thumb_block(Cpu& cpu, uint32_t pc, uint32_t max_insns,
                                   IrBlock* out) {
  const uint32_t n = max_insns < uint32_t(IR_MAX_INSNS) ? max_insns
                                                       : uint32_t(IR_MAX_INSNS);
  return translate(&cpu, nullptr, n, pc, /*thumb=*/true, out);
}

uint32_t jit_translate_block(Cpu& cpu, uint32_t pc, uint32_t max_words,
                             IrBlock* out) {
  return cpu.thumb ? jit_translate_thumb_block(cpu, pc, max_words, out)
                   : jit_translate_arm_block(cpu, pc, max_words, out);
}

/* An operation that sets the guest's flags needs the bits AArch64 got wrong put
 * back.  `mask` is which NZCV bits to overwrite, and the correct value is a field of
 * `src`: either the CPSR the guest came in with (kept in SCR2 by the entry code) or
 * a bit of the operand that went through the barrel shifter - ARM takes C from the
 * shifter, where AArch64 takes it from the sum.
 *
 * Two cases need no repair at all: an unshifted `ANDS`/`BICS`, whose C AArch64
 * leaves alone exactly as ARM does, never happens (C comes out of the shifter as 0
 * in both, so only V differs - see below); and ADC/SBC, which are refused.  */
struct FlagPatch {
  uint32_t mask, src, lsb, width;
  /* A patch whose value is a translation-time constant instead of a field of a
   * register: bit 31 of a rotated immediate, which no register is guaranteed to
   * hold once the operation has run. */
  bool is_const = false;
  uint32_t bits = 0;
};

uint32_t shifter_carry_bit(const IrInsn& in);

int flag_patches(const IrInsn& in, uint32_t saved, uint32_t shifted_src,
                 FlagPatch out[2]) {
  if (!in.setflags) return 0;
  const bool unshifted =
      in.b_is_imm || (in.shift_type == arm64::SH_LSL && in.shift_amt == 0);
  int n = 0;
  switch (in.op) {
    case IR_AND: case IR_BIC: case IR_ORR: case IR_EOR: case IR_MVN:
    case IR_MOV: case IR_MOVI: case IR_LSL: case IR_LSR: case IR_ASR: case IR_ROR:
      /* C is taken from the barrel shifter, never from the host: AArch64 defines it
       * for a 64-bit shift (so `LSL #17` on a value that fits in 32 bits has
       * nothing to carry out), and an emulator is free to leave it at zero for
       * ANDS/BICS.  ARM's rule is one bit of the operand, which is cheap to state
       * and always right.  V is never written by an ARM logical operation at all. */
      if (in.b_is_imm) {
        /* An immediate operand: the rotate field decides.  Nothing rotated means
         * C is left alone - and V always is, whatever the rotation. */
        if (in.imm_rot == 0) { out[n++] = {0x30000000u, saved, 28, 2u}; return n; }
        out[n++] = {0x10000000u, saved, 28, 1u};
        FlagPatch c{};
        c.mask = 0x20000000u;                  /* C, and only C             */
        c.is_const = true;
        c.bits = (in.imm >> 31) ? c.mask : 0u; /* already positioned        */
        out[n++] = c;
        return n;
      }
      if (unshifted) { out[n++] = {0x30000000u, saved, 28, 2u}; return n; }
      out[n++] = {0x10000000u, saved, 28, 1u};
      out[n++] = {0x20000000u, shifted_src, shifter_carry_bit(in), 1u};
      return n;
    /* Nothing to repair for the arithmetic operations: unlike the logical ones,
     * `ADDS`/`SUBS`/`RSBS` take C from the carry out of *sum*, not from the barrel
     * shifter, whether or not the second operand was shifted - and that is what
     * AArch64 does with the same operand, so N, Z, C and V all already agree. */
    default:
      return 0;
  }
}

uint32_t shifter_carry_bit(const IrInsn& in) {
  /* A left shift loses its top bit, so C is the one that fell off the *bottom* of
   * the result: bit 32-n.  Every right-ward operation - LSR, ASR, ROR - loses bits
   * off the bottom, so C is bit n-1, and RRX's is bit 0.  A 32-bit LSR/ASR therefore
   * wants bit 31, which is why the front end refuses those instead of modelling a
   * shift field that cannot hold 32. */
  if (in.shift_type == SH_RRX) return 0u;
  if (in.shift_type == arm64::SH_LSL) return (32u - in.shift_amt) & 31u;
  if (in.shift_amt == 0) return 0u;
  return in.shift_amt - 1u;
}

bool jit_emit_arm64(const IrBlock& blk, bool with_entry_exit,
                    std::vector<uint32_t>* out) {
  arm64::Buf b;
  if (with_entry_exit) b.prologue();
  for (uint32_t i = 0; i < blk.n; ++i) {
    const IrInsn& in = blk.in[i];
    /* An operation with no destination (TST/TEQ/CMP/CMN) still has to put its value
     * somewhere to raise N and Z from it, and XZR would make that meaningless. */
    const bool no_dst = in.dst == IR_NO_DST;   /* see the note at the end of the body */
    const uint32_t rd = no_dst ? arm64::XZR : reg_of(in.dst);
    const uint32_t vdst = no_dst ? arm64::SCR1 : reg_of(in.dst);
    const uint32_t ra = reg_of(in.a);
    const uint32_t sh = in.shift_amt;
    /* The value the operation shifts: Rm for most forms, but the front end moves
     * the source of a MOV/MVN/shift into `a` and leaves `b` alone, so the choice
     * depends on the operation.  `operand_orig` stays the *guest* register: the C
     * bit ARM puts in the carry flag is one of its bits, whether or not the shifted
     * value itself has been materialised into a scratch. */
    const bool mov_like = in.op == IR_MVN || in.op == IR_MOV || in.op == IR_MOVI ||
                          in.op == IR_LSL || in.op == IR_LSR || in.op == IR_ASR ||
                          in.op == IR_ROR || in.op == IR_RRX;
    const uint32_t operand_orig = mov_like ? ra : reg_of(in.b);
    /* Which bits the operation is going to get wrong has to be decided, and the
     * flags snapshot taken, before the operation itself: `TST Wd, Wd` and friends
     * overwrite everything, so by then the bits ARM leaves alone are gone.  SCR3 is
     * free through the body of a block - the entry code only needs it in the
     * epilogue, and no accepted instruction writes a guest register there. */
    FlagPatch fp[2];
    const int np = flag_patches(in, arm64::SCR3, operand_orig, fp);
    if (np) b.mrs_nzcv(arm64::SCR3);
    /* An RRX operand - ARM's ROR #0 - inserts the *current* carry at bit 31, which
     * no AArch64 shift field says (QEMU reads a `ROR #0` shift field as a rotate by
     * zero, i.e. as nothing at all), so it is done explicitly and the operation then
     * sees a plain register.  A rotated immediate is likewise materialised for the
     * forms that cannot take one.  The MOV family spells its operand differently -
     * `MOV Rd, Rm, RRX` has its own IR op - but MVN is an operation with a second
     * operand as far as AArch64 is concerned, so it needs the fold too. */
    const bool rrx = !in.b_is_imm && in.shift_type == SH_RRX &&
                     (!mov_like || in.op == IR_MVN);
    const bool fold_imm = in.b_is_imm && in.op != IR_MOVI && in.op != IR_ADD &&
                          in.op != IR_SUB && in.op != IR_RSB;
    uint32_t operand = operand_orig;
    uint32_t st = rrx ? uint32_t(arm64::SH_LSL) : uint32_t(in.shift_type);
    uint32_t amt = rrx ? 0u : sh;
    if (rrx) { b.mov_rrx(arm64::SCR1, operand_orig); operand = arm64::SCR1; }
    else if (in.shift_type == SH_RRX) return false;   /* nowhere to put an RRX */
    else if (fold_imm) { b.mov_imm(arm64::SCR1, in.imm); operand = arm64::SCR1; }
    switch (in.op) {
      case IR_MOVI:
        /* `MOVS Rd, #imm` (and every Thumb `movs Rd, #imm8`): the value, then
         * N and Z from it, and C and V repaired below like any other logical. */
        b.mov_imm(rd, in.imm);
        if (in.setflags) b.set_nz_from(rd);
        break;
      case IR_MOV:
        b.mov_reg(vdst, ra);
        if (in.setflags) b.set_nz_from(vdst);
        break;
      case IR_ADD:
      case IR_SUB: {
        /* Arithmetic: C and V come out of the sum in both ISAs, so nothing has to
         * be repaired, and a shifted operand can stay in the shift field unless it
         * is a rotate, which the field has no room for. */
        const bool is_sub = in.op == IR_SUB;
        if (in.b_is_imm && in.imm <= 0xFFFu && amt == 0) {
          if (is_sub) b.sub_imm(rd, ra, in.imm, in.setflags);
          else b.add_imm(rd, ra, in.imm, in.setflags);
        } else if (in.b_is_imm) {
          b.mov_imm(arm64::SCR1, in.imm);
          if (is_sub) b.sub_shifted(rd, ra, arm64::SCR1, arm64::SH_LSL, 0,
                                    in.setflags);
          else b.add_shifted(rd, ra, arm64::SCR1, arm64::SH_LSL, 0, in.setflags);
        } else if (in.shift_type == arm64::SH_ROR && !rrx) {
          b.shift(arm64::SCR1, operand_orig, arm64::SH_ROR, sh);
          if (is_sub) b.sub_shifted(rd, ra, arm64::SCR1, arm64::SH_LSL, 0,
                                    in.setflags);
          else b.add_shifted(rd, ra, arm64::SCR1, arm64::SH_LSL, 0, in.setflags);
        } else if (is_sub) {
          b.sub_shifted(rd, ra, operand, st, amt, in.setflags);
        } else {
          b.add_shifted(rd, ra, operand, st, amt, in.setflags);
        }
        break;
      }
      case IR_RSB:
        /* Rd = Op2 - Rn: ARM shifted *Op2*, but AArch64's shift field belongs to the
         * subtrahend, so a shifted Op2 is materialised first and the subtraction
         * then reads a plain register.  An immediate becomes the value subtracted
         * from, which is only the same operation while no flags are wanted. */
        if (in.b_is_imm && !in.setflags && in.imm <= 0xFFFu) {
          b.mov_imm(rd, in.imm);
          b.sub_shifted(rd, rd, ra, arm64::SH_LSL, 0, false);
        } else if (in.b_is_imm) {
          return false;
        } else if (rrx || amt == 0) {
          b.sub_shifted(rd, operand, ra, arm64::SH_LSL, 0, in.setflags);
        } else {
          b.shift(arm64::SCR1, operand_orig, st, amt);
          b.sub_shifted(rd, arm64::SCR1, ra, arm64::SH_LSL, 0, in.setflags);
        }
        break;
      /* AND and BIC have a flag-setting form with a destination of their own; ORR
       * and EOR do not exist with S at all, so those write the value and raise N and
       * Z with `TST Wd, Wd`. */
      case IR_AND:
        b.and_shifted(rd, ra, operand, st, amt, in.setflags);
        break;
      case IR_BIC:
        b.bic_shifted(rd, ra, operand, st, amt, in.setflags);
        break;
      case IR_ORR:
        b.orr_shifted(vdst, ra, operand, st, amt);
        if (in.setflags) b.set_nz_from(vdst);
        break;
      case IR_EOR:
        b.eor_shifted(vdst, ra, operand, st, amt);
        if (in.setflags) b.set_nz_from(vdst);
        break;
      case IR_MVN:
        b.orn_shifted(vdst, arm64::XZR, operand, st, amt);
        if (in.setflags) b.set_nz_from(vdst);
        break;
      case IR_NEG:
        b.neg_shifted(rd, operand, st, amt, in.setflags);
        break;
      case IR_ADC:
        /* `ADCS` is a real AArch64 instruction, and its C is the sum's carry-out
         * with the flag register's C added in - ARM's rule whenever the shifter
         * supplied no carry itself (which the front end guarantees here). */
        b.adc(rd, ra, operand, in.setflags);
        break;
      case IR_SBC:
        /* Same story: AArch64's SBC subtracts `NOT(C)` and defines C as the
         * inverse of the borrow, exactly ARM's.  An immediate operand has already
         * been materialised into SCR1 above (minus the S bit, which is why the
         * immediate form used to be refused here). */
        b.sbc(rd, ra, operand, in.setflags);
        break;
      case IR_RSC:
        /* Rd = Op2 - Rn - NOT(C): AArch64's SBC with the operands exchanged, and
         * it wants no flag repair at all - N, Z, C and V come out of the sum the
         * same way ARM defines them, with S or without.  SBC takes plain
         * registers, so an immediate or a shifted Op2 is materialised first, the
         * same move the RSB case makes just above. */
        if (in.b_is_imm && in.setflags) return false;   /* front end refuses too */
        if (in.b_is_imm) {
          b.mov_imm(rd, in.imm);
          b.sbc(rd, rd, ra);
        } else if (rrx || amt == 0) {
          b.sbc(rd, operand, ra, in.setflags);
        } else {
          b.shift(arm64::SCR1, operand_orig, st, amt);
          b.sbc(rd, arm64::SCR1, ra, in.setflags);
        }
        break;
      case IR_LSL:
      case IR_LSR:
      case IR_ASR:
      case IR_ROR: {
        const uint32_t k = in.op == IR_LSL ? arm64::SH_LSL
                         : in.op == IR_LSR ? arm64::SH_LSR
                         : in.op == IR_ASR ? arm64::SH_ASR : arm64::SH_ROR;
        b.shift(vdst, ra, k, sh);
        if (in.setflags) b.set_nz_from(vdst);
        break;
      }
      case IR_RRX:
        if (in.setflags) return false;   /* MOVS ...RRX also copies C into V */
        b.mov_rrx(rd, ra);
        break;
      default: return false;
    }
    for (int k = 0; k < np; ++k)
      if (fp[k].is_const) b.flags_patch_imm(fp[k].mask, fp[k].bits);
      else b.flags_patch(fp[k].mask, (uint32_t)__builtin_ctz(fp[k].mask),
                         fp[k].src, fp[k].lsb, fp[k].width);
  }
  if (with_entry_exit) b.epilogue(blk.next_pc, blk.dirty);
  *out = b.w;
  return true;
}

}  // namespace brain::arm926
