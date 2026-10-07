/* The JIT's intermediate representation.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A block is a straight-line run of guest instructions translated into ops over
 * a *host* register file that shadows guest R0-R12 while the block runs.  The
 * shape is deliberately small: the frontend bails (and the block ends) at any
 * branch, any access to R13-R15, anything that reads or writes CPSR beyond the
 * NZCV flags, and any instruction that touches memory - a guest load or store
 * needs the MMU and the abort path, which a block must not duplicate.
 *
 * What is left is the bulk of a real loop body: ARM data-processing, including
 * the barrel shifter and the S forms.  Flags are carried in the *host* NZCV for
 * the whole block, because the guest's S bit and AArch64's S bit mean the same
 * thing, so `adds`/`subs`/`ands` need no fixup at all and only the block's entry
 * and exit move flags between the host register and the CPSR word.
 *
 * The IR exists so the ARM-specific logic is written once: `frontend.cpp` turns
 * `ArmDec` into these ops and a backend only has to walk them.  AArch64 is the
 * backend that exists today (emit_arm64.h); on any other host the words below are
 * still built, so the frontend stays testable without a code generator.
 */
#ifndef BRAIN_JIT_IR_H
#define BRAIN_JIT_IR_H

#include <cstdint>

namespace brain::arm926 {

/* Guest registers are addressed by number; IR_NPC is the value the block leaves
 * in R15, i.e. the address of the first guest instruction it did not translate. */
enum : uint8_t {
  IR_REG_BASE = 0,       /* the pointer to the shadowed register file  */
  IR_NPC = 16,
  IR_NVIRTUAL = 17,
};

enum IrOp : uint8_t {
  IR_MOV = 0,            /* dst = src                                   */
  IR_MOVI,               /* dst = imm                                   */
  IR_ADD, IR_SUB, IR_RSB, IR_ADC, IR_SBC, IR_AND, IR_ORR, IR_EOR, IR_BIC,
  IR_MVN, IR_NEG,
  IR_LSL, IR_LSR, IR_ASR, IR_ROR,   /* dst = src <<|>> amt             */
  IR_RRX,               /* dst = (C << 31) | (src >> 1): ARM's rotate-right
                         * through the carry, which is what a shift field of
                         * ROR #0 or LSR #0 means. */
  /* Appended at the end so the codes above keep the numbers a debugging session
   * has already printed: RSC is the one data-processing operation whose second
   * operand is the *minuend* (`dst = Op2 - Rn - NOT(C)`), i.e. SBC with the two
   * register operands exchanged - which is exactly how both backends spell it. */
  IR_RSC,
  IR_COUNT
};

/* One op.  Operands are virtual-register numbers, and a shift on the second
 * operand is applied by the backend as a shifted-register form rather than a
 * second instruction, because AArch64 has exactly that field. */
struct IrInsn {
  IrOp op;
  uint8_t dst;         /* 255 = the flags are the only result (CMP/TST)  */
  uint8_t a;
  uint8_t b;
  bool b_is_imm;
  bool setflags;
  uint8_t shift_type;  /* SH_* : 0 = lsl, 1 = lsr, 2 = asr, 3 = ror      */
  uint8_t shift_amt;
  uint32_t imm;
  /* The rotate field of an ARM immediate operand.  `imm` holds the *value* the
   * operation sees (already rotated), so this is what tells a backend whether
   * the rotation still supplies a carry for a flag-setting form: it does when
   * this is non-zero (and the bit is bit 31 of `imm`), and C is left alone when
   * it is zero. */
  uint8_t imm_rot;
};

enum { IR_NO_DST = 255, IR_MAX_INSNS = 64 };

/* A translated block: the ops, plus what the entry has to restore so a backend
 * can build its prologue/epilogue without looking at the guest again. */
struct IrBlock {
  IrInsn in[IR_MAX_INSNS];
  uint32_t n = 0;
  uint32_t start_pc = 0;      /* address of the first translated insn      */
  uint32_t next_pc = 0;       /* where execution resumes                   */
  uint32_t guest_insns = 0;   /* how many retired, for the block counter   */
  uint32_t dirty = 0;         /* bitmask of guest regs written             */
  bool truncated = false;     /* stopped by IR_MAX_INSNS, not by a boundary */
  const char* stop = nullptr;  /* why the block ended, for --jit tracing   */

  bool empty() const { return n == 0; }
  bool push(const IrInsn& i) {
    if (n >= IR_MAX_INSNS) { truncated = true; return false; }
    in[n++] = i;
    return true;
  }
};

inline const char* ir_op_name(IrOp op) {
  switch (op) {
    case IR_MOV: return "mov";
    case IR_MOVI: return "movi";
    case IR_ADD: return "add";
    case IR_SUB: return "sub";
    case IR_RSB: return "rsb";
    case IR_ADC: return "adc";
    case IR_SBC: return "sbc";
    case IR_RSC: return "rsc";
    case IR_AND: return "and";
    case IR_ORR: return "orr";
    case IR_EOR: return "eor";
    case IR_BIC: return "bic";
    case IR_MVN: return "mvn";
    case IR_NEG: return "neg";
    case IR_LSL: return "lsl";
    case IR_LSR: return "lsr";
    case IR_ASR: return "asr";
    case IR_ROR: return "ror";
    case IR_RRX: return "rrx";
    default: return "?";
  }
}

}  // namespace brain::arm926

#endif
