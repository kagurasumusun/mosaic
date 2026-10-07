/* Guest ARM -> JIT IR, and IR -> AArch64.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The translation reads the same decoder the interpreter does, so the two cannot
 * disagree about what an encoding means: `decode_arm` fills an `ArmDec`, and this
 * file only has to decide whether the result is something a block may contain and
 * then re-shape it.  That is the whole reason the IR exists - the ARM side is
 * written once, next to the decoder, and a backend never sees an instruction.
 */
#include "arm926/jit/frontend.h"

#include <cstring>

#include "arm926/decode.h"
#include "arm926/jit/emit_arm64.h"

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
 * which case the caller ends the block *before* this instruction. */
bool translate_dp(const ArmDec& d, IrInsn* out) {
  /* `ArmDec::cond` is the Thumb branch field; for ARM words the decoder reports a
   * failed condition as AOP_SATISFY and executes nothing, so the only thing left to
   * check here is that the block is not asked to run an instruction the dispatcher
   * would skip: a conditional one would have to be predicated instead. */
  if (((d.insn >> 28) & 0xFu) != 0xEu) return false;              /* conditional: dispatcher's job */
  if (d.op2 == OP2_REG_RS) return false;         /* shift by a register          */
  /* Only R0-R13 are addressable through the ABI's X1-X14 window; SP/LR/PC (and
   * banked registers in general) stay the interpreter's, and the dispatcher is what
   * handles them, so a block ends here instead. */
  if (d.rd > 12u || d.rn > 12u || d.rm > 12u || d.ra > 12u) return false;
  if (d.op2 != OP2_NONE && d.op2 != OP2_IMM && d.op2 != OP2_REG &&
      d.op2 != OP2_REG_SHIFT)
    return false;

  /* Immediates and shifts are fine while they do not set the flags: the ARM C bit
   * for a rotated immediate comes from the rotator (and is left alone when the
   * rotation is zero), which AArch64's `ADDS #imm` does not model - it produces the
   * arithmetic carry instead.  A shift that sets flags has the same problem via the
   * `ORR Rd, WZR, Rm, #n` form, whose C is 0 rather than the bit rotated out.
   * Register operands with S, by contrast, are an exact match: AArch64 copies the
   * shifter carry the same way ARM does.  Anything else stops here and runs in the
   * dispatcher, which is correct - just not fast. */
  /* A flag-setting operation with a *rotated immediate* is refused: ARM takes C
   * from the rotator (and leaves it alone when the rotation is zero), which is not
   * what `ADDS #imm` computes, and no amount of shifting the materialised value
   * recovers the difference.  `MOV`/`MVN` with a shifted *register* and S is fine -
   * the value, `TST Wd, Wd`, and the snapshot repair below give exactly ARM's rules
   * for it - but a shift by a *register* also copies C into V, so that stays with
   * the dispatcher. */
  if (d.s && d.op2 == OP2_IMM) return false;

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
    /* ADC/SBC take their carry input from the flags, and AArch64 has no
     * flag-setting form of either: `ADCS Rd, Rn, Rm` would need the carry *and*
     * overflow out of a three-operand sum, which is not one instruction.  So only
     * the flag-free versions are translated, and the flag versions end the block -
     * the dispatcher runs them. */
    case DP_ADC:
      if (d.op2 != OP2_REG || d.s) return false;
      in.op = IR_ADC;
      break;
    case DP_SBC:
      if (d.op2 != OP2_REG || d.s) return false;
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

/* One instruction word as the *fetch* would see it: identity-mapped RAM is read
 * straight out of the array, anything else has to walk the TLB the way
 * `Cpu::prepare_fetch()` does, and a translation that does not resolve simply
 * ends the block (the dispatcher runs the instruction, and takes its abort if
 * that is what the fetch does).  Reading physical memory at `pc` instead would
 * happily translate bytes the guest never sees.  If `prepare_fetch()` changes,
 * this has to change with it. */
bool fetch_word(Cpu& cpu, uint32_t va, uint32_t* out) {
  if (cpu.ram_fast && ((va - cpu.ram_gbase) & ~cpu.ram_mask) == 0) {
    std::memcpy(out, cpu.ram + (va & cpu.ram_mask), 4);   /* VA == PA here */
    return true;
  }
  TlbEntry* e = cpu.tlb_lookup(va & ~0xFFFu, MF_FETCH | MF_SZ32);
  if (!e) return false;
  *out = cpu.phys_read(e->gpa + (va & 0xFFFu), 4);
  return true;
}

/* `words != nullptr` translates a caller-supplied array (a test's); otherwise
 * the words are fetched from `cpu` one at a time, so a block that ends at its
 * first instruction costs one fetch and not a whole window of them. */
uint32_t translate(Cpu* cpu, const uint32_t* words, uint32_t n, uint32_t pc,
                   IrBlock* out) {
  out->n = 0;
  out->start_pc = pc;
  out->next_pc = pc;
  out->guest_insns = 0;
  out->dirty = 0;
  out->truncated = false;
  out->stop = "empty";
  uint32_t i = 0;
  for (; i < n; ++i) {
    uint32_t word;
    if (words) {
      word = words[i];
    } else if (!fetch_word(*cpu, pc + i * 4u, &word)) {
      out->stop = i ? "fetch" : "unsupported";
      break;
    }
    ArmDec d{};
    const u32 op = decode_arm(word, &d);
    if (op != AOP_DP) { out->stop = i ? "not data-processing" : "unsupported"; break; }
    IrInsn in{};
    if (!translate_dp(d, &in)) {
      out->stop = i ? "outside the translated subset" : "unsupported";
      break;
    }
    if (in.dst != IR_NO_DST) out->dirty |= 1u << in.dst;
    if (!out->push(in)) { out->stop = "block too long"; ++i; break; }
    out->guest_insns++;
    out->next_pc = pc + (i + 1) * 4u;
    out->stop = "budget";
  }
  return i;
}

}  // namespace

uint32_t jit_translate_arm_words(const uint32_t* words, uint32_t n, uint32_t pc,
                                 IrBlock* out) {
  return translate(nullptr, words, n, pc, out);
}

uint32_t jit_translate_arm_block(Cpu& cpu, uint32_t pc, uint32_t max_words,
                                 IrBlock* out) {
  const uint32_t n = max_words < uint32_t(IR_MAX_INSNS) ? max_words
                                                       : uint32_t(IR_MAX_INSNS);
  return translate(&cpu, nullptr, n, pc, out);
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
    case IR_MOV: case IR_LSL: case IR_LSR: case IR_ASR: case IR_ROR:
      /* C is taken from the barrel shifter, never from the host: AArch64 defines it
       * for a 64-bit shift (so `LSL #17` on a value that fits in 32 bits has
       * nothing to carry out), and an emulator is free to leave it at zero for
       * ANDS/BICS.  ARM's rule is one bit of the operand, which is cheap to state
       * and always right.  V is never written by an ARM logical operation at all. */
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
        if (in.setflags) return false;   /* MOVS #imm: C comes from the rotation */
        b.mov_imm(rd, in.imm);
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
        if (in.setflags) return false;      /* refused by the front end too */
        b.adc(rd, ra, operand);
        break;
      case IR_SBC:
        if (in.setflags) return false;
        b.sbc(rd, ra, operand);
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
      b.flags_patch(fp[k].mask, (uint32_t)__builtin_ctz(fp[k].mask), fp[k].src,
                    fp[k].lsb, fp[k].width);
  }
  if (with_entry_exit) b.epilogue(blk.next_pc, blk.dirty);
  *out = b.w;
  return true;
}

}  // namespace brain::arm926
