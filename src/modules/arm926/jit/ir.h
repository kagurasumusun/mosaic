/* The JIT's intermediate representation.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A block is a straight-line run of guest instructions translated into ops over
 * a *host* register file that shadows guest R0-R12 while the block runs, ended
 * either by the translation budget or by a branch.  The shape is deliberately
 * small: the frontend bails (and the block ends) at anything that reads or
 * writes CPSR beyond the NZCV flags, at a guest access the machine cannot let a
 * block perform, and at every instruction whose semantics would need a second
 * copy of the exception model.  A guest load or store is translated only when
 * the machine says the access is plain RAM (MMU off, no alignment checking),
 * and a block that turns out to be wrong about the *address* leaves the block
 * again instead of guessing (see IR_LD/IR_ST below).
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
  /* Memory and control flow.  A block may contain these only while the module
   * says it may (the frontend checks the machine state that makes them safe);
   * both ends of the block are then still a clean handoff, because an access a
   * block must not perform and a branch both *exit* rather than guess. */
  IR_LD,                /* load:  dst = memory at the address below          */
  IR_ST,                /* store: memory at the address below = dst          */
  IR_BR,                /* the block ends here: a branch, taken or not       */
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

  /* ---- IR_LD / IR_ST: what a guest access means ---------------------------
   *
   * `dst` is the guest register loaded (IR_LD) or stored (IR_ST), `a` the base
   * register and `b` the index register.  The address is
   *
   *   mem_reg == MEM_ABS   : `imm`                    (pc-relative, folded)
   *   mem_reg == MEM_IMM   : R(a) +- `imm`
   *   mem_reg == MEM_REG   : R(a) +- (R(b) <shift_type> #shift_amt)
   *
   * `mem_sub` says the offset is subtracted, `mem_size` is 1, 2 or 4 bytes, and
   * `mem_signed` sign-extends a load (LDRSB/LDRSH).  `mem_post` is what the
   * *access* reads: 0 = the effective address (the base with the offset
   * applied), 1 = the base itself, with the offset left for the writeback.
   * `mem_wb` is whether the base is updated afterwards - to the effective
   * address for a pre-indexed form, to base +- offset for a post-indexed one.
   * The two are separate because the frontend drops the writeback (not the
   * addressing form) for a load whose destination is its own base, which is the
   * one case where the interpreter's rule and the encoding's differ.
   * `mem_shadow` means R(a) is R13/R14: those live in the shadow register file
   * rather than in the allocator's window, so the backend reads them from there
   * (and a writeback writes them back there).  The frontend refuses every
   * UNPREDICTABLE combination (a load whose destination is also its written-back
   * base, R15 as an operand) rather than giving them a meaning.
   *
   * A backend may only perform one of these when the machine has told it the
   * access cannot fault and cannot reach a device - which is a *translation*
   * decision, carried by IrBlock::uses_mem plus the window in
   * IrBlock::mem_gbase/mem_mask - and it must be able to leave the block before
   * an access that falls outside what it can do (see the exit protocol in
   * jit/emit_x64.cpp): the dispatcher then runs that one instruction itself.
   */
  uint8_t mem_size;
  uint8_t mem_wb;
  uint8_t mem_post;
  uint8_t mem_sub;
  uint8_t mem_reg;
  uint8_t mem_signed;
  uint8_t mem_shadow;

  /* ---- IR_BR: the last instruction of a branch-terminated block ----------
   *
   * `cond` is the ARM condition code (0xE = always).  A direct branch puts its
   * *absolute* target in `imm`; `br_reg` instead names the target register in
   * `b`.  `br_link` writes `link` - the address after the branch word - to the
   * architectural R14, which lives in the shadow: only on the taken path, so a
   * conditional BL that does not execute really does write nothing.
   * `blk.next_pc` is the fall-through - the address after the branch word - and
   * the emitter picks between the fall-through and the target at run time by
   * evaluating `cond` on the guest's flags. */
  uint8_t cond;
  uint8_t br_link;
  uint8_t br_reg;
  uint32_t link;         /* IR_BR: the value a BL/BLX leaves in R14 */

  /* ---------------------------------------------------------- cycle cost --
   * What the interpreter charges for this instruction (kIssue plus whatever
   * the instruction adds), accumulated over the block by the frontend: `cyc` is
   * the running total *through* this instruction.  It rides in the IR because
   * the cycle model belongs to the guest module and the backends live below it;
   * a block that leaves early reports the total it reached. */
  uint16_t cyc;
  /* The cumulative cost when a *conditional branch* is not taken (an issue slot
   * and nothing else: the interpreter charges a taken branch and only the issue
   * slot for one that falls through).  Unused for every other op. */
  uint16_t cyc_nt;
};

/* `mem_reg` / `mem_wb` values (see the field comments above). */
enum : uint8_t {
  MEM_ABS = 0, MEM_IMM = 1, MEM_REG = 2,      /* IR_LD / IR_ST addressing    */
  MEM_WB_NONE = 0, MEM_WB_YES = 1,            /* ...and the writeback        */
};

enum { IR_NO_DST = 255, IR_MAX_INSNS = 64 };

/* A translated block: the ops, plus what the entry has to restore so a backend
 * can build its prologue/epilogue without looking at the guest again. */
struct IrBlock {
  IrInsn in[IR_MAX_INSNS];
  uint32_t n = 0;
  uint32_t start_pc = 0;      /* address of the first translated insn      */
  uint32_t next_pc = 0;       /* where a straight-line exit resumes        */
  /* The address after the last translated word: the block's *source* range,
   * which is what an invalidation and the code-page marking care about.  For
   * everything but an unconditional branch this equals `next_pc`. */
  uint32_t code_end = 0;
  uint32_t guest_insns = 0;   /* how many retired, for the block counter   */
  uint32_t dirty = 0;         /* bitmask of guest regs written             */
  uint32_t cycles = 0;        /* the whole block's cycle cost              */
  /* Memory: `uses_mem` is set when the block contains a guest access, and the
   * window below is what the backend bakes into it.  It is the identity RAM
   * window as of translation (`Cpu::ram_gbase`/`ram_mask`): the dispatcher
   * refuses to enter a memory block whose window no longer matches, which is
   * what makes the baked constants safe (see jit_cache_enter). */
  bool uses_mem = false;
  uint32_t mem_gbase = 0, mem_mask = 0;
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
    case IR_LD: return "ld";
    case IR_ST: return "st";
    case IR_BR: return "br";
    default: return "?";
  }
}

}  // namespace brain::arm926

#endif
