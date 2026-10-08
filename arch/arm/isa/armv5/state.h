/* brain-emu -- the ARMv5TEJ instruction set architecture: architectural state.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This is the *ISA* layer (arch/arm/isa/armv5/): everything the ARMv5TEJ
 * architecture defines, and nothing that belongs to a particular core.  It
 * holds the architectural register state (IsaState), the PSR/flag/mode model,
 * the condition codes, the exception model's vector table, and the register
 * banking rules - everything any ARMv5TEJ core (ARM926EJ-S, ARM946E-S, an
 * ARM7TDMI wired to the same engine, ...) shares.  The instruction decoders
 * (decode*.h/.cpp), the execution semantics (interp*.cpp), the disassembler
 * (disasm.cpp) and the barrel shifter (ops.h) live next to it and are all
 * written against IsaState alone.
 *
 * What this layer deliberately does NOT know:
 *
 *   - memory.  Loads and stores go through the IsaEnv the core fills in
 *     (memory.h): the ISA engine asks "read this address" and the core answers
 *     with its own translation, bus and endianness rules.  A core with no MMU,
 *     a core with a different MMU, or a unit test with a flat array all supply
 *     their own.
 *   - coprocessors / system control.  MCR/MRC are answered through IsaEnv's
 *     coproc callbacks; CP15 itself (the ARM926EJ-S's system control
 *     coprocessor, its TLB, its caches) is a core, in
 *     arch/arm/cpu/classic/arm9/arm926ej-s/.
 *   - run control: instruction budgets, halting, stop requests, JIT
 *     notification and statistics accounting beyond the counters the engine
 *     itself keeps.  Those are the embedding core's policy.
 *
 * A core embeds the ISA by inheritance: `struct Cpu : public IsaState` (the
 * ARM926EJ-S does exactly that), so the engine, the core's own code and the
 * JIT all operate on one object with one register file.
 *
 * Layout notes for the embedding core: the register file, the pipeline view
 * (cur/npc/insn), CPSR and its V mirror sit at the top of IsaState because the
 * JIT dispatcher copies them per block entry; the bank array follows.  The
 * "hot region" discipline of the old monolithic core (a couple of cache lines
 * at the top, banked registers swapped in/out on mode change so every
 * data-path access is a single base+disp load) is preserved here.  Nothing in
 * the ISA layer is addressed by offset - generated code talks to the shadow
 * buffer (arch/arm/cpu/classic/arm9/arm926ej-s/jit/abi.h) - so the layout may
 * evolve freely.
 *
 * References: ARM DDI 0100I (ARM ARM v5), chapters A2 (programmers' model)
 * and A4/A5 (instruction encodings).
 */
#ifndef BRAIN_ISA_ARMV5_STATE_H
#define BRAIN_ISA_ARMV5_STATE_H

#include "brainemu/types.h"

namespace brain::arm::isa::armv5 {

/* Barrel-shifter result: value plus the carry the shift produced. */
/* SH_RRX is not an encoding: the decoder substitutes it for the ROR field whose
 * amount is zero, which ARM state spells as RRX. */
enum : u32 { SH_LSL = 0, SH_LSR = 1, SH_ASR = 2, SH_ROR = 3, SH_RRX = 4 };
struct Shifted {
  u32 val;
  u32 carry;
};

/* ================================================================ modes == */

enum : u32 {
  MODE_USR = 0x10,
  MODE_FIQ = 0x11,
  MODE_IRQ = 0x12,
  MODE_SVC = 0x13,
  MODE_ABT = 0x17,
  MODE_UND = 0x1B,
  MODE_SYS = 0x1F,
};

enum : u32 {
  BANK_USR = 0,   /* USR + SYS  : r0-r14          */
  BANK_FIQ,       /* r8_fiq-r14_fiq               */
  BANK_IRQ,       /* r13_irq, r14_irq             */
  BANK_SVC,       /* r13_svc, r14_svc             */
  BANK_ABT,       /* r13_abt, r14_abt             */
  BANK_UND,       /* r13_und, r14_und             */
  BANK_COUNT = 6,
};

/* ================================================================= CPSR == */

enum : u32 {
  CPSR_T   = 1u << 5,   /* Thumb state                                    */
  CPSR_F   = 1u << 6,   /* FIQ disable                                    */
  CPSR_I   = 1u << 7,   /* IRQ disable                                    */
  CPSR_A   = 1u << 8,   /* alignment check enable                         */
  CPSR_E   = 1u << 9,   /* BE8: big-endian data                           */
  CPSR_J   = 1u << 24,  /* Jazelle                                        */
  CPSR_Q   = 1u << 27,  /* sticky overflow (ARMv5E)                       */
  CPSR_V   = 1u << 28,
  CPSR_C   = 1u << 29,
  CPSR_Z   = 1u << 30,
  CPSR_N   = 1u << 31,
};

enum : u32 {
  CPSR_FLAGS   = 0xF0000000u,
  CPSR_NVQC    = 0xF8000000u,
  CPSR_MODE_M  = 0x0000001Fu,
  CPSR_CTRL_M  = 0x000000F0u,
  /* What "MSR CPSR, #all" may write on ARM926EJ-S (ARMv5 B2.9.4). */
  CPSR_WRITABLE = CPSR_NVQC | CPSR_CTRL_M | CPSR_MODE_M | CPSR_J,
};

/* Field-select mask nibbles used by MSR{cond} <CPSR|SPSR>_<fields>. */
enum : u32 { MSR_C = 1u << 0, MSR_X = 1u << 1, MSR_S = 1u << 2, MSR_F = 1u << 3 };

enum : u32 {
  COND_EQ = 0x0, COND_NE = 0x1, COND_CS = 0x2, COND_CC = 0x3,
  COND_MI = 0x4, COND_PL = 0x5, COND_VS = 0x6, COND_VC = 0x7,
  COND_HI = 0x8, COND_LS = 0x9, COND_GE = 0xA, COND_LT = 0xB,
  COND_GT = 0xC, COND_LE = 0xD, COND_AL = 0xE, COND_NV = 0xF,
};

enum : u32 {
  VEC_RESET = 0x00, VEC_UNDEF = 0x04, VEC_SWI = 0x08, VEC_PABORT = 0x0C,
  VEC_DABORT = 0x10, VEC_RSVD = 0x14, VEC_IRQ = 0x18, VEC_FIQ = 0x1C,
  VEC_HIGH_BASE = 0xFFFF0000u,
};

enum : u32 {
  EXC_NONE = 0, EXC_RESET, EXC_UNDEF, EXC_SWI, EXC_PABORT, EXC_DABORT,
  EXC_IRQ, EXC_FIQ, EXC_COUNT,
};

/* Banked register set: the mode's R8..R14 (indexed by register number, so
 * r[13]/r[14] are the banked SP/LR and r[8..12] the FIQ-only set) plus its
 * SPSR.  USR/SYS share BANK_USR; the SPSR of USR/SYS reads as zero (there is
 * none).  R0-R7 are never banked: they name the same physical registers in
 * every mode, which is why a mode change must leave them untouched. */
struct Bank {
  u32 r[16];
  u32 spsr;
};

/* ============================================================ IsaState ==
 *
 * The architectural state of an ARMv5TEJ processor: the 16 registers of the
 * current mode (R15 is the PC, kept with its pipeline bias - see pc_bias),
 * the pipeline view, CPSR with its V mirror, the Thumb bit, the privilege
 * mirror, the bank array, the retired-instruction/cycle counters the
 * engines charge, and the pending-exception state.
 *
 * Three fields at the end are *core-maintained behaviour policy* the engine
 * reads but never derives: l4 (SCTLR.L4 - "loads to PC also load the T bit",
 * which on this architecture generation comes from system control, not from
 * the ISA), jazelle_trap and strict_unpredictable (emulator strictness
 * options).  The embedding core keeps them current.
 */
struct IsaState {
  /* ---- hot: copied per block entry by the JIT dispatcher -------------- */
  u32 regs[16];  /* R0..R15 of the *current* mode; R15 keeps its pipeline bias */
  u32 npc;       /* address of the next instruction to fetch                    */
  u32 cur;       /* address of the instruction being executed                   */
  u32 insn;      /* the instruction word / halfword being executed              */
  u32 cpsr;
  u32 vflag;     /* mirror of CPSR.V (host flags have no cheap V writer)        */
  u32 thumb;     /* the T bit, mirrored: the state bit, not just CPSR.T         */
  u32 priv;      /* 1 in a privileged mode (mirror of the mode field)           */
  Bank bank[BANK_COUNT];
  u32 bank_idx;  /* which bank regs[] currently mirrors                         */

  /* ---- counters -------------------------------------------------------- */
  u64 cycles;    /* guest cycles charged under the shared CycleModel            */
  u64 insns;     /* guest instructions retired                                  */

  /* ---- pending exception (architectural) -------------------------------
   * exc/exectrap: the exception the engine is in the middle of raising.
   * The core takes it at the next step boundary (step_one) or immediately
   * from the fetch path; exc_fsr/exc_far carry the fault status/address
   * for the abort kinds.  exc is cleared when the exception is entered. */
  u32 exc;
  u32 exectrap;
  u32 exc_fsr;
  u32 exc_far;

  /* ---- core-maintained behaviour policy (see above) -------------------- */
  u32 l4;                 /* SCTLR.L4: LDR-to-PC loads the T bit                 */
  u32 jazelle_trap;       /* trap BXJ instead of honouring Jazelle state     */
  u32 strict_unpredictable; /* undefined encodings raise UNDEF (vs ignore)   */

  /* ---- state accessors (inline: they sit on every data path) ----------- */

  /* Keep CPSR.T, the thumb mirror and the pipeline in step.  The T bit is
   * architectural state, not just a CPSR bit: the decoder that runs next
   * depends on it. */
  ALWAYS_INLINE void set_thumb(u32 t) {
    thumb = t ? 1u : 0u;
    cpsr = (cpsr & ~CPSR_T) | (thumb ? CPSR_T : 0u);
  }

  ALWAYS_INLINE u32 reg(u32 n) const { return regs[n]; }
  ALWAYS_INLINE u32 pc_pipeline() const { return regs[15]; }
  ALWAYS_INLINE u32 mode() const { return cpsr & CPSR_MODE_M; }
  ALWAYS_INLINE bool thumb_mode() const { return thumb != 0; }
  ALWAYS_INLINE u32 cur_insn_size() const { return thumb ? 2 : 4; }
  ALWAYS_INLINE u32 carry() const { return (cpsr >> 29) & 1; }
  ALWAYS_INLINE u32 overflow() const { return (cpsr >> 28) & 1; }
  ALWAYS_INLINE u32 qflag() const { return (cpsr >> 27) & 1; }
  ALWAYS_INLINE void set_q() { cpsr |= CPSR_Q; }

  /* Condition evaluation (A4.3).  V is read through the vflag mirror, which
   * the flag helpers keep exact (host flag registers have no cheap overflow
   * writer, which is why the mirror exists at all).  COND_NV (the "never" of
   * ARMv5, the BLX-imm cond field) is handled by the decoder, not here. */
  ALWAYS_INLINE bool condition_true(u32 cond) const {
    switch (cond) {
      case COND_EQ: return (cpsr & CPSR_Z) != 0;
      case COND_NE: return (cpsr & CPSR_Z) == 0;
      case COND_CS: return (cpsr & CPSR_C) != 0;
      case COND_CC: return (cpsr & CPSR_C) == 0;
      case COND_MI: return (cpsr & CPSR_N) != 0;
      case COND_PL: return (cpsr & CPSR_N) == 0;
      case COND_VS: return vflag != 0;
      case COND_VC: return vflag == 0;
      case COND_HI: return (cpsr & (CPSR_C | CPSR_Z)) == CPSR_C;
      case COND_LS: return (cpsr & (CPSR_C | CPSR_Z)) != CPSR_C;
      case COND_GE: return ((((cpsr >> 31) & 1) ^ vflag) == 0);
      case COND_LT: return ((((cpsr >> 31) & 1) ^ vflag) != 0);
      case COND_GT: return ((cpsr & CPSR_Z) == 0) && ((((cpsr >> 31) & 1) ^ vflag) == 0);
      case COND_LE: return ((cpsr & CPSR_Z) != 0) || ((((cpsr >> 31) & 1) ^ vflag) != 0);
      default: return true;   /* AL, and NV is treated as AL on ARMv5 */
    }
  }

  /* Flag helpers.  The V mirror exists because host flag registers have no
   * cheap overflow writer; the ISA layer keeps it exact so both engines and
   * the disassembler agree on the guest state. */
  ALWAYS_INLINE void set_nz(u32 res) {
    cpsr = (cpsr & ~(CPSR_N | CPSR_Z)) | (res & CPSR_N) | (res == 0 ? u32(CPSR_Z) : 0u);
  }
  ALWAYS_INLINE void set_cpsr_flags(u32 n, u32 z, u32 c, u32 vv) {
    cpsr = (cpsr & ~CPSR_FLAGS) | (n ? u32(CPSR_N) : 0u) | (z ? u32(CPSR_Z) : 0u) |
           (c ? u32(CPSR_C) : 0u) | (vv ? u32(CPSR_V) : 0u);
    vflag = vv ? 1 : 0;
  }

  /* Width of the instruction at `cur` and the bias R15 reads as (the
   * pipeline: ARM reads PC as cur+8, Thumb as cur+4). */
  ALWAYS_INLINE u32 insn_size() const { return thumb ? 2u : 4u; }
  ALWAYS_INLINE u32 pc_bias() const { return thumb ? 4u : 8u; }

  /* ---- banking and PC writes (state.cpp) ------------------------------- */

  /* Register write with the R15 rules: a write to R15 is a branch - the
   * pipeline view is rebuilt and (ARMv5T) the low bits select Thumb when
   * the SCTLR.L4 behaviour is off. */
  void set_reg(u32 i, u32 v);
  /* Move the live register window to another mode's bank. */
  void switch_mode(u32 new_mode);
  void flush_bank();
  void load_bank(u32 idx);
  u32 bank_for_mode(u32 mode) const;
};

/* A mode value the architecture defines (used to validate MSR mode writes). */
bool valid_mode(u32 m);

}  // namespace brain::arm::isa::armv5

#endif  /* BRAIN_ISA_ARMV5_STATE_H */
