/* brain-emu -- ARMv5TEJ ISA: the interpreter engine (ARM and Thumb states).
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The execution semantics of every ARMv5TEJ instruction, written against the
 * architectural state (IsaState) and the environment contract (IsaEnv) only:
 * memory, coprocessor transfers, exception entry and the JIT hooks all go
 * through the environment the embedding core fills in (memory.h), so this file
 * knows nothing about any particular core - the ARM926EJ-S, an ARM946E-S, or
 * a unit test with a flat array can all drive it.
 *
 * Semantics sources, in order of authority for this ISA:
 *   - ARM DDI 0100I  (the ARM ARM for ARMv4/v5T/v5E/v6) -- reference/DDI01001.pdf
 *   - ARM DDI 0198E  (ARM926EJ-S TRM)                   -- reference/DDI0198E_...pdf
 * Everything the two engines must agree on (barrel shifter, flag updates,
 * saturating arithmetic, unaligned-access permission, cycle charges, block
 * transfer address sequences) goes through ops.h, so the interpreter and the
 * JIT cannot drift apart.
 */
#include <cstdio>
#include <cstring>

#include "arch/arm/isa/armv5/decode.h"
#include "arch/arm/isa/armv5/exceptions.h"
#include "arch/arm/isa/armv5/interp.h"
#include "arch/arm/isa/armv5/ops.h"

namespace brain::arm::isa::armv5 {

/* ------------------------------------------------------------ operand2 -- */

ALWAYS_INLINE Shifted arm_op2(const IsaState& s, const ArmDec& d) {
  const u32 cin = (s.cpsr >> 29) & 1;
  switch (d.op2) {
    case OP2_IMM: {
      /* value = ROR(imm8, 2*rot); carry = rot ? value<31> : C unchanged */
      if (d.rot == 0) return Shifted{d.imm12 & 0xFF, cin};
      const u32 v = ror32(d.imm12 & 0xFF, (d.rot & 0xF) * 2);
      return Shifted{v, (v >> 31) & 1};
    }
    case OP2_REG:
      return Shifted{s.regs[d.rm], cin};
    case OP2_REG_SHIFT:
      return shift_arm(s.regs[d.rm], d.sh, d.amt, cin);
    default: {
      /* Shifted by a register: the amount is Rs<7:0> (B2.2.3).  A zero there is a
       * shift by zero - no value change and C left alone - because the #0
       * re-encodings are a property of the immediate field, see decode_op2_reg.
       * Going through shift_arm() is the point: this is the same primitive the JIT
       * front end reads, so the two engines cannot disagree about a shift. */
      const u32 amt = s.regs[d.rs] & 0xFF;
      return shift_arm(s.regs[d.rm], d.sh, amt, cin);
    }
  }
}

/* ---------------------------------------------------------- addressing -- */

u32 arm_ldst_offset(const IsaState& s, const ArmDec& d) {
  if (d.op2 == OP2_NONE) return d.imm12;
  return arm_op2(s, d).val;
}

/* Block transfer (LDM/STM) address sequence, ARM DDI 0100I A5.4.2..A5.4.5. */
void arm_block_addrs(const IsaState& s, const ArmDec& d, u32 n, u32* start, u32* wb) {
  const u32 base = s.regs[d.rn];
  const u32 span = n * 4;
  if (d.u) *start = base + (d.p ? 4 : 0);
  else *start = base - (d.p ? span : span - 4);
  *wb = base + (d.u ? span : 0u - span);
}

/* --------------------------------------------------------- CPSR restore -- */

/* LDR/LDM/MOV into R15 with S set: CPSR <- SPSR (mode switch included). */
ALWAYS_INLINE void restore_cpsr_from_spsr(IsaState& s, IsaEnv& env) {
  const u32 spsr = s.bank[s.bank_idx].spsr;
  const u32 new_mode = spsr & CPSR_MODE_M;
  if (new_mode != (s.cpsr & CPSR_MODE_M)) {
    /* s.switch_mode() flushes the current bank and loads the target bank */
    s.switch_mode(new_mode);
  }
  s.cpsr = spsr;
  s.vflag = (spsr >> 28) & 1;
  s.set_thumb((spsr & CPSR_T) ? 1 : 0);
  s.priv = (s.cpsr & CPSR_MODE_M) != MODE_USR;
  env.sync_caches(env.ctx);
}

/* A write to R15 that is not a plain branch.  `from_load` marks a load whose
 * result feeds the PC (LDR, LDRH, LDM/POP with R15 in the list): the L4
 * policy bit gives that case its own rule - "0 = loads to PC set the T bit,
 * 1 = loads to PC do not set the T bit (ARMv4 behaviour)" (DDI 0198E Table
 * 2-11, CP15 c1[15] on the ARM926EJ-S, mirrored into IsaState::l4 by the
 * core) - and L4 resets to 0, so an odd load target does change state.
 * Every other writer of the PC (MOV, ORR, ADD, MUL, MRS, MRC) ignores bits[1:0]
 * and cannot change state on this core; ARMv6+ made those behave like BX, so the
 * golden emulator disagrees there and the generator keeps such encodings out of
 * the corpus instead of the core following it.  A PC restored together with an
 * SPSR restore (MOVS-into-PC, LDM-with-S) takes its state from the SPSR and
 * passes from_load = false.  `s.cur` and R15 are refreshed by the next fetch,
 * which keeps the pipeline rule s.regs[15] == s.cur + (s.thumb ? 4 : 8) true
 * everywhere. */
ALWAYS_INLINE void load_pc(IsaState& s, u32 value, bool from_load) {
  const bool set_t = from_load && !s.l4;
  const bool to_thumb = set_t ? (value & 1u) != 0 : s.thumb != 0;
  s.npc = to_thumb ? (value & ~1u) : (value & ~3u);
  if (set_t && to_thumb != (s.thumb != 0)) {
    s.set_thumb(to_thumb ? 1u : 0u);
    s.cpsr = (s.cpsr & ~(CPSR_T | CPSR_J)) | (to_thumb ? u32(CPSR_T) : 0u);
  }
  s.cycles += CycleModel::kPcLoad;
}

/* ------------------------------------------------------------- MSR ----- */

void apply_msr(IsaState& s, u32 mask, u32 val, bool spsr, IsaEnv& env) {
  /* Which CPSR bits each field select may touch on ARMv5 (B2.9.4):
   *   F -> bits 31:24 (N,Z,C,V,Q at 27 and J at 24 are the writable ones)
   *   S -> bits 23:16 (all reserved on ARM926EJ-S)
   *   X -> bits 15:8  (A at 8, E at 9)
   *   C -> bits  7:0  (I, F, T and the mode field; T is not writable here)  */
  u32 allowed = 0;
  if (mask & MSR_F) allowed |= 0xF9000000u;
  if (mask & MSR_S) allowed |= 0x00000000u;
  if (mask & MSR_X) allowed |= 0x00000300u;
  if (mask & MSR_C) allowed |= 0x000000DFu;
  allowed &= ~CPSR_T;

  if (spsr) {
    s.bank[s.bank_idx].spsr = (s.bank[s.bank_idx].spsr & ~allowed) | (val & allowed);
    return;
  }
  if ((val & allowed & CPSR_J) && s.jazelle_trap) {
    /* Entering Jazelle state is not implemented (docs/ISA-COVERAGE.md): the
     * attempt takes an Undefined Instruction exception instead. */
    raise_undef(s, s.insn);
    return;
  }
  const u32 new_cpsr = (s.cpsr & ~allowed) | (val & allowed);
  const u32 new_mode = new_cpsr & CPSR_MODE_M;
  if (new_mode != (s.cpsr & CPSR_MODE_M) && valid_mode(new_mode)) {
    /* s.switch_mode() banks the old mode's R8-R14/FIQ regs and installs the new
     * mode's bank; the mode field it writes is the same one new_cpsr holds. */
    s.cpsr = new_cpsr;
    s.switch_mode(new_mode);
  } else {
    s.cpsr = new_cpsr;
  }
  s.vflag = (s.cpsr >> 28) & 1;
  s.priv = (s.cpsr & CPSR_MODE_M) != MODE_USR;
  env.notify_jit(env.ctx, "msr");
}

/* ======================================================== exec_arm ===== */

/* Is this ARM word a branch whose *condition check* is the branch decision?
 * B/BL (opcode 101), BX and BLX(register).  Only the branch counters use this,
 * and they need it because a failed condition never reaches the decoder
 * (A6.1.1 makes the instruction a NOP), so "not taken" has to be recognised
 * from the encoding. */
static ALWAYS_INLINE bool arm_branch_word(u32 w) {
  if (((w >> 25) & 7u) == 5u) return true;               /* B, BL            */
  const u32 k = w & 0x0FFFFFF0u;
  return k == 0x012FFF10u || k == 0x012FFF30u;           /* BX, BLX Rm       */
}

void exec_arm(IsaState& s, u32 insn_word, IsaEnv& env) {
  s.insns++;
  env.stats->insns_interp++;
  s.cycles += CycleModel::kIssue;

  if (BRAIN_UNLIKELY(!s.condition_true(insn_word >> 28))) {        /* NOP: A6.1.1 */
    if (arm_branch_word(insn_word)) env.stats->branch_not++;
    return;
  }

  ArmDec d;
  decode_arm(insn_word, &d);
  exec_arm_dec(s, d, env);
}

/* Execute an already-decoded instruction.  Both states share this path: the
 * Thumb decoder fills the same ArmDec, so a Thumb instruction takes exactly
 * the ARM semantics of the equivalent encoding -- which is what the hardware
 * does (A6: most Thumb formats are "the ARM instruction with the same name").
 * The instruction count and the issue cycle are charged by the caller. */
void exec_arm_dec(IsaState& s, const ArmDec& d, IsaEnv& env) {
  const u32 op = d.op;
  /* `s.insn` is read from the state: an undefined-instruction report has to
   * quote the word that is stored there, so no local copy is kept. */

  switch (op) {

  /* -------------------------------------------------- data processing -- */
  case AOP_DP: {
    const Shifted sh = arm_op2(s, d);
    /* Thumb state treats the PC as a *word-aligned* operand of data processing: the
     * value used is Align(PC, 4) = (s.cur + 4) & ~3, which is what makes `ADR Rd,
     * #imm8*4` (T1, decoded as ADD Rd, PC, #imm) land on a word boundary even for a
     * halfword-aligned instruction, and matches the `s.regs[15] & ~3` the load/store
     * paths use for their base.  ARM state uses s.regs[15] = s.cur + 8 unmasked; only
     * the T1 groups put 15 in d.rn, so the 32-bit forms keep the plain +4 rule. */
   const u32 a = (d.rn == 15 && s.thumb) ? (s.regs[15] & ~3u) : s.regs[d.rn];
    const u32 b = sh.val;
    u32 res = 0, nc = 0, nv = 0;
    switch (d.dp) {
      case DP_AND: res = a & b; break;
      case DP_EOR: res = a ^ b; break;
      case DP_BIC: res = a & ~b; break;
      case DP_ORR: res = a | b; break;
      case DP_MOV: res = b; break;
      case DP_MVN: res = ~b; break;
      case DP_TST: res = a & b; break;
      case DP_TEQ: res = a ^ b; break;
      case DP_SUB: case DP_CMP: {
        res = a - b;
        nc = (a >= b);
        nv = (((a ^ b) & (a ^ res)) >> 31);
        break;
      }
      case DP_RSB: {
        res = b - a;
        nc = (b >= a);
        nv = (((b ^ a) & (b ^ res)) >> 31);
        break;
      }
      case DP_ADD: case DP_CMN: {
        const u64 t = (u64)a + (u64)b;
        res = (u32)t;
        nc = (u32)(t >> 32);
        nv = (((~(a ^ b)) & (a ^ res)) >> 31);
        break;
      }
      case DP_ADC: {
        const u64 t = (u64)a + (u64)b + (u64)((s.cpsr >> 29) & 1);
        res = (u32)t;
        nc = (u32)(t >> 32);
        nv = (((~(a ^ b)) & (a ^ res)) >> 31);
        break;
      }
      case DP_SBC: {
        /* C is NOT(borrow) over the *whole* expression `a - b - NOT(C)`
         * (A4.1.61): testing `a >= b + NOT(C)` in 32 bits instead is wrong,
         * because `b + NOT(C)` wraps for b == 0xFFFFFFFF and C == 0 and the
         * wrapped 0 makes a borrow look like no borrow.  The carry-in has to
         * take part in the comparison, so the difference is formed in 64 bits
         * - the borrow is then simply the sign of the exact difference. */
        const u32 c = 1 - ((s.cpsr >> 29) & 1);
        const u64 t = (u64)a - (u64)b - (u64)c;
        res = (u32)t;
        nc = ((t >> 32) == 0);
        nv = (((a ^ b) & (a ^ res)) >> 31);
        break;
      }
      case DP_RSC: {
        /* Same rule with the operands the other way round: `b - a - NOT(C)`. */
        const u32 c = 1 - ((s.cpsr >> 29) & 1);
        const u64 t = (u64)b - (u64)a - (u64)c;
        res = (u32)t;
        nc = ((t >> 32) == 0);
        nv = (((b ^ a) & (b ^ res)) >> 31);
        break;
      }
      default: break;
    }

    if (d.s) {
      if (dp_flags_only(d.dp)) {
        if (d.dp == DP_TST || d.dp == DP_TEQ) set_logical_flags(s, res, sh.carry);
        else set_arith_flags(s, res, nc, nv);
        return;
      }
      if (d.rd == 15) {
        /* "ADDS/MOVS PC, ...": CPSR <- SPSR, ALU flags discarded, PC <- res */
        restore_cpsr_from_spsr(s, env);
        load_pc(s, res);
        return;
      }
      if (dp_is_logical(d.dp)) set_logical_flags(s, res, sh.carry);
      else set_arith_flags(s, res, nc, nv);
    }
    if (d.rd != 15) s.regs[d.rd] = res;
    else load_pc(s, res);
    if (d.op2 == OP2_REG_RS) s.cycles += CycleModel::kShiftByReg;
    return;
  }

  /* ----------------------------------------------------- MRS / MSR ---- */
  case AOP_MRS: {
    if (d.psr && s.mode() == MODE_USR) { raise_undef(s, s.insn); return; }  /* SPSR in USR */
    const u32 val = d.psr ? s.bank[s.bank_idx].spsr : s.cpsr;
    if (d.rd == 15) load_pc(s, val);
    else s.regs[d.rd] = val;
    return;
  }
  case AOP_MSR: {
    if (s.mode() == MODE_USR) { raise_undef(s, s.insn); s.cycles += CycleModel::kUndefCycles; return; }
    u32 val;
    if (d.op2 == OP2_IMM) val = arm_op2(s, d).val;
    else val = s.regs[d.rm];
    apply_msr(s, d.mask, val, d.psr != 0, env);
    return;
  }

  /* ------------------------------------------------------- multiply --- */
  case AOP_MUL: {
    u32 res = s.regs[d.rm] * s.regs[d.rs];
    if (d.kind) res += s.regs[d.rn];
    if (d.s) set_nz_only(s, res);
    if (d.rd == 15) {
      if (s.strict_unpredictable) { raise_undef(s, s.insn); return; }
      load_pc(s, res);
    } else {
      s.regs[d.rd] = res;
    }
    if (d.kind) s.cycles += CycleModel::kMulMla;
    return;
  }
  case AOP_MULL: {
    const bool us = (d.kind & 2) != 0;      /* signed                    */
    const bool acc = (d.kind & 1) != 0;     /* accumulate (UMLAL/SMLAL)  */
    const u32 lo = d.rd, hi = d.ra;
    if (lo == 15 || hi == 15 || lo == hi) {
      if (s.strict_unpredictable) { raise_undef(s, s.insn); return; }
    }
    const u64 p = mul_arm(s.regs[d.rm], s.regs[d.rs], us, acc,
                          acc ? (((u64)s.regs[hi] << 32) | s.regs[lo]) : 0);
    const u32 rlo = (u32)p, rhi = (u32)(p >> 32);
    if (d.s) {
      s.cpsr = (s.cpsr & ~(CPSR_N | CPSR_Z)) | (rhi & CPSR_N) |
             ((rlo | rhi) == 0 ? u32(CPSR_Z) : 0u);
    }
    if (lo != 15) s.regs[lo] = rlo;
    if (hi != 15) s.regs[hi] = rhi;
    s.cycles += CycleModel::kMulLong;
    return;
  }
  case AOP_MULH: {
    /* SMLAxy / SMULxy: x (bit 0) selects Rm's half, y (bit 1) selects Rs's.
     * SMULxy leaves Q alone; SMLAxy sets it when the 32-bit accumulate
     * saturates (A3.5.3). */
    const u32 res_half = (u32)((s64)((s32)(s16)hw_operand(s.regs[d.rm], d.kind & 1)) *
                              (s64)((s32)(s16)hw_operand(s.regs[d.rs], (d.kind >> 1) & 1)));
    if (d.ra == 0x1F) {                 /* SMULxy: no accumulate, no flags   */
      if (d.rd == 15) load_pc(s, res_half);
      else s.regs[d.rd] = res_half;
      return;
    }
    const s64 acc = (s64)(s32)res_half + (s64)(s32)s.regs[d.ra];
    u32 qq = 0;
    const u32 res = saturate32(acc, &qq);
    if (d.rd == 15) load_pc(s, res);
    else s.regs[d.rd] = res;
    if (qq) s.set_q();          /* Q is sticky: only raised here */
    return;
  }
  case AOP_MULW: {
    /* SMLAWy / SMULWy: (word x halfword) >> 16 */
    const u32 y = d.kind & 1;
    const bool word_only = (d.kind >> 1) & 1;
    const s64 prod = ((s64)(s32)s.regs[d.rm] *
                      (s64)((s32)(s16)hw_operand(s.regs[d.rs], y))) >> 16;
    if (word_only) {
      if (d.rd == 15) load_pc(s, (u32)prod);
      else s.regs[d.rd] = (u32)prod;
      return;
    }
    u32 qq = 0;
    const u32 res = saturate32(prod + (s64)(s32)s.regs[d.ra], &qq);
    if (d.rd == 15) load_pc(s, res);
    else s.regs[d.rd] = res;
    if (qq) s.set_q();          /* Q is sticky: only raised here */
    return;
  }
  case AOP_MULHAL: {
    /* SMLALxy: 32-bit accumulate into RdLo:RdHi, Q set on overflow */
    const s64 prod = (s64)((s32)(s16)hw_operand(s.regs[d.rm], d.kind & 1)) *
                     (s64)((s32)(s16)hw_operand(s.regs[d.rs], (d.kind >> 1) & 1));
    /* The pair is assembled in the unsigned domain and reinterpreted: a left
     * shift of a negative s64 is undefined, and the bits are the same. */
    const s64 acc = (s64)((((u64)s.regs[d.ra]) << 32) | s.regs[d.rd]);
    const s64 res = prod + acc;
    /* A3.5.3: "SMULxy and SMLALxy do not affect any flags" -- no Q, no NZ. */
    if (d.rd != 15) s.regs[d.rd] = (u32)(u64)res;
    if (d.ra != 15) s.regs[d.ra] = (u32)((u64)res >> 32);
    s.cycles += CycleModel::kMulLong;
    return;
  }

  /* ------------------------------------------------- saturating / CLZ -- */
  case AOP_QD: {
    const u32 a = s.regs[d.rn], b = s.regs[d.rm];
    u32 q = 0, res = 0;
    switch (d.kind) {
      case 0: res = add_saturate(a, b, &q); break;
      case 1: res = sub_saturate(a, b, &q); break;
      case 2: {
        u32 q2 = 0;
        const u32 dbl = half_sat_double(b, &q2);
        res = add_saturate(a, dbl, &q);
        q |= q2;
        break;
      }
      default: {
        u32 q2 = 0;
        const u32 dbl = half_sat_double(b, &q2);
        res = sub_saturate(a, dbl, &q);
        q |= q2;
        break;
      }
    }
    if (d.rd == 15) load_pc(s, res);
    else s.regs[d.rd] = res;
    if (q) s.set_q();            /* Q is sticky: only raised here */
    return;
  }
  case AOP_CLZ: {
    const u32 x = s.regs[d.rm];
    const u32 res = x ? (u32)__builtin_clz(x) : 32u;
    if (d.rd == 15) load_pc(s, res);
    else s.regs[d.rd] = res;
    return;
  }

  /* ------------------------------------------------------------- SWP -- */
  case AOP_SWP: {
    const u32 addr = s.regs[d.rn];
    /* B2.6.2: SWP and SWPB must always be word aligned. */
    if (addr & 3) { raise_dabort(s, env, FS926_ALIGN, addr); return; }
    const u32 sz = d.b ? 1u : 4u;
    const u32 v = env.mem_read(env.ctx, addr, sz, (d.b ? MF_SZ8 : MF_SZ32) | MF_SWP);
    if (BRAIN_UNLIKELY(s.exectrap)) return;
    env.mem_write(env.ctx, addr, s.regs[d.rm], sz, (d.b ? MF_SZ8 : MF_SZ32) | MF_SWP | MF_WRITE);
    if (BRAIN_UNLIKELY(s.exectrap)) return;
    if (d.rd == 15) load_pc(s, v);
    else s.regs[d.rd] = v;
    s.cycles += CycleModel::kSwpExtra;
    return;
  }

  /* ------------------------------------------------- LDR / STR (32/8) -- */
  case AOP_LDRST: {
    const u32 off = arm_ldst_offset(s, d);
    /* A5.3.1/A6.5: with R15 as the base the address is the PC value read by
     * this instruction with bits[1:0] treated as zero -- which matters only in
     * Thumb state, where the instruction itself may be halfword aligned. */
    const u32 base = d.rn == 15 ? (s.regs[15] & ~3u) : s.regs[d.rn];
    const bool wb = (d.am == AM_IMM_PRE_WB || d.am == AM_REG_PRE_WB ||
                     d.am == AM_IMM_POST || d.am == AM_REG_POST);
    const u32 ea = (d.am == AM_IMM_POST || d.am == AM_REG_POST)
                       ? base
                       : (d.u ? base + off : base - off);
    const u32 next = d.u ? base + off : base - off;
    const u32 sz = d.b ? 1u : 4u;
    /* TRM 3.5.1: with CP15 c1[A] set the MMU faults on any unaligned *word*
     * or *halfword* access, whatever the addressing form; only byte accesses
     * and instruction fetches are exempt.  `unaligned_ok` therefore no longer
     * suppresses the check - it is what the byte forms already get for free. */
    const u32 flags = (d.b ? MF_SZ8 : MF_SZ32) | (d.b ? u32(MF_UNAL) : 0u);

    if (d.l) {
      const u32 v = env.mem_read(env.ctx, ea, sz, flags);
      if (BRAIN_UNLIKELY(s.exectrap)) return;      /* abort: Rn not updated */
      if (d.rd == 15) load_pc(s, v, /*from_load=*/true);
      else s.regs[d.rd] = v;
    } else {
      /* STR R15 stores the instruction address + 8, i.e. the pipeline value. */
      const u32 src = d.rd;
      const u32 data = (src == 15) ? s.regs[15] : s.regs[src];
      env.mem_write(env.ctx, ea, data, sz, flags | MF_WRITE);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
    }
    if (wb) {
      /* "LDR Rn,[Rn,#x]!" leaves Rn with the loaded value (UNPREDICTABLE;
       * the ARM926EJ-S keeps the loaded value -- match that).            */
      if (!(d.l && d.rn == d.rd)) s.regs[d.rn] = next;
    }
    s.cycles += CycleModel::kAccess;
    return;
  }

  /* --------------------------------- LDRH/STRH/LDRSB/LDRSH (extra LDST) -- */
  case AOP_LDRSTH: {
    const u32 off = arm_ldst_offset(s, d);
    const u32 base = d.rn == 15 ? (s.regs[15] & ~3u) : s.regs[d.rn];
    const bool wb = (d.am == AM_IMM_PRE_WB || d.am == AM_REG_PRE_WB ||
                     d.am == AM_IMM_POST || d.am == AM_REG_POST);
    const u32 ea = (d.am == AM_IMM_POST || d.am == AM_REG_POST) ? base
                                                                : (d.u ? base + off : base - off);
    const u32 next = d.u ? base + off : base - off;
    /* H=1 -> 16 bit, H=0 with S=1 -> 8 bit (signed byte) */
    const u32 sz = d.h ? 2u : 1u;
    const u32 flags = (sz == 2 ? MF_SZ16 : MF_SZ8) | (sz == 1 ? u32(MF_UNAL) : 0u);
    if (d.l) {
      u32 v = env.mem_read(env.ctx, ea, sz, flags);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
      if (d.signed_) v = sz == 2 ? (u32)(s32)(s16)v : (u32)(s32)(s8)v;
      if (d.rd == 15) load_pc(s, v, /*from_load=*/true);
      else s.regs[d.rd] = v;
    } else {
      /* A5.3.3: in this encoding space a store names its source in bits[15:12]
       * (the Rd field) and only the halfword form exists. */
      env.mem_write(env.ctx, ea, s.regs[d.rd], 2, MF_SZ16 | MF_WRITE);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
    }
    if (wb && !(d.l && d.rn == d.rd)) s.regs[d.rn] = next;
    s.cycles += CycleModel::kAccess;
    return;
  }

  /* ----------------------------------------------------- LDRD / STRD -- */
  case AOP_LDRD: {
    const u32 off = arm_ldst_offset(s, d);
    const u32 base = d.rn == 15 ? (s.regs[15] & ~3u) : s.regs[d.rn];
    const bool wb = (d.am == AM_IMM_PRE_WB || d.am == AM_REG_PRE_WB ||
                     d.am == AM_IMM_POST || d.am == AM_REG_POST);
    const u32 ea = (d.am == AM_IMM_POST || d.am == AM_REG_POST) ? base
                                                                : (d.u ? base + off : base - off);
    const u32 next = d.u ? base + off : base - off;
    if (d.rd == 15 || (d.rd & 1)) { raise_undef(s, s.insn); return; }
    const u32 flags = MF_SZ32;
    if (!d.kind) {
      const u32 v0 = env.mem_read(env.ctx, ea, 4, flags);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
      const u32 v1 = env.mem_read(env.ctx, ea + 4, 4, flags);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
      s.regs[d.rd] = v0;
      s.regs[d.rd + 1] = v1;
    } else {
      env.mem_write(env.ctx, ea, s.regs[d.rd], 4, flags | MF_WRITE);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
      env.mem_write(env.ctx, ea + 4, s.regs[d.rd + 1], 4, flags | MF_WRITE);
      if (BRAIN_UNLIKELY(s.exectrap)) return;
    }
    if (wb) s.regs[d.rn] = next;
    s.cycles += CycleModel::kAccess;
    return;
  }

  /* --------------------------------------------------- LDM / STM ----- */
  case AOP_LDM: case AOP_STM: {
    const bool load = (op == AOP_LDM);
    const u32 list = d.list;
    const u32 n = (u32)__builtin_popcount(list);
    if (n == 0) {
      /* An empty register list is UNPREDICTABLE (A5.4); ARMv5 cores perform no
       * access and, for the writeback forms, leave Rn alone or update it by
       * zero.  We do the latter: it is what an ARM926EJ-S does and it keeps
       * LDM/STM side-effect free. */
      return;
    }
    u32 start, wbv;
    arm_block_addrs(s, d, n, &start, &wbv);

    /* User-bank transfer (A5.4: S=1 always for LDM with PC? no: for LDM, S=1
     * with PC means "restore CPSR from SPSR"; S=1 without PC means user bank.
     * For STM, S=1 always transfers the user bank; and STM with R15 in the
     * list from a privileged mode always stores the user R15. */
    bool user_bank;
    bool restore_flags = false;
    if (load) {
      if (d.kind && s.mode() == MODE_USR) { raise_undef(s, s.insn); return; }
      if (list & (1u << 15)) restore_flags = d.kind != 0;
      user_bank = (d.kind != 0) && !(list & (1u << 15));
    } else {
      user_bank = d.kind != 0;
      if (user_bank && s.mode() == MODE_USR) { raise_undef(s, s.insn); return; }
    }
    const u32 saved_bank = s.bank_idx;
    if (user_bank) { s.flush_bank(); s.load_bank(BANK_USR); }

    u32 addr = start;
    for (u32 i = 0; i < 16; ++i) {
      if (!(list & (1u << i))) continue;
      if (load) {
        const u32 v = env.mem_read(env.ctx, addr, 4, MF_SZ32);
        if (BRAIN_UNLIKELY(s.exectrap)) {
          if (user_bank) s.load_bank(saved_bank);
          return;                        /* abort: Rn is not written back   */
        }
        s.regs[i] = v;
      } else {
        /* STM of R15 always stores the *pipeline* value of R15 (i.e. +12
         * here) except in the user-bank case, where the user R15 is stored. */
        const u32 v = (i == 15 && !user_bank) ? s.regs[15] : s.regs[i];
        env.mem_write(env.ctx, addr, v, 4, MF_SZ32 | MF_WRITE);
        if (BRAIN_UNLIKELY(s.exectrap)) {
          if (user_bank) {
            s.bank[BANK_USR].r[15] = s.regs[15];
            s.load_bank(saved_bank);
          }
          return;
        }
      }
      addr += 4;
    }
    if (user_bank) {
      if (load) s.bank[BANK_USR].r[15] = s.regs[15];
      s.load_bank(saved_bank);
    }
    if (d.w) s.regs[d.rn] = wbv;
    if (load && (list & (1u << 15))) {
      const u32 newpc = s.regs[15];
      if (restore_flags) restore_cpsr_from_spsr(s, env);
      load_pc(s, newpc, /*from_load=*/!restore_flags);
    }
    s.cycles += CycleModel::kAccess * (n - 1);
    return;
  }

  /* --------------------------------------------------- branches ------ */
  case AOP_B: {
    /* Every B/BL/BLX transfers control except the Thumb long-branch setup half
     * (kind 4), which only writes LR - so the "taken" counter sits here, once,
     * instead of at each of the four exits below. */
    if (d.kind != 4) env.stats->branch_taken++;
    /* Thumb state: the long branch is a *pair of independent halfwords* in
     * ARMv5T (A7.1.17), so each half is its own instruction here.  kind 4 is
     * the H==10 setup half, 5 the H==11 (BL) half and 6 the H==01 (BLX to ARM)
     * half.  They work off LR, not off the PC, which is what lets an
     * assembler place them anywhere in a ±4MB window. */
    if (d.kind >= 4) {
      if (d.kind == 4) { s.regs[14] = s.regs[15] + d.imm24; return; }
      const u32 target = s.regs[14] + d.imm24;
      s.regs[14] = (s.cur + 2) | 1u;
      if (d.kind == 6) {                       /* BLX: switch to ARM state     */
        s.npc = target & ~3u;
        s.set_thumb(0);
        s.cpsr &= ~(CPSR_T | CPSR_J);
        s.cycles += CycleModel::kBranch + CycleModel::kPcLoad;
        return;
      }
      s.npc = target & ~1u;
      s.cycles += CycleModel::kBranch;
      return;
    }
    /* Branches read the PC as `s.cur + 8` (ARM state), which is exactly what
     * s.regs[15] holds, and LR = s.cur + 4 for BL / s.cur & ~3 for BLX. */
    const u32 target = s.regs[15] + d.imm24;
    if (d.kind == 1) s.regs[14] = s.cur + 4;                    /* BL        */
    /* kind 3 = Thumb B<cond>, already filtered by exec_thumb(). */
    if (d.kind == 2) {                                       /* BLX #imm */
      /* The link carries the state to return *to*.  The caller is in ARM state,
       * so bit[0] stays clear: `BX R14`, or `POP {PC}` with L4 = 0 (where a load
       * takes its state from bit[0]), then lands back in ARM.  Setting bit[0]
       * here would send the return straight back into the Thumb callee. */
      s.regs[14] = s.cur + 4;
      s.set_thumb(1);
      s.cpsr |= CPSR_T;
      s.cpsr &= ~CPSR_J;
      /* The target already carries bit[1] from H, so only bit[0] is dropped -
       * masking to a word boundary would discard the half that selects between
       * two adjacent Thumb instructions. */
      s.npc = target & ~1u;
      s.cycles += CycleModel::kBranch + CycleModel::kPcLoad;
      return;
    }
    /* A Thumb branch target is only halfword aligned: applying the ARM rule here
     * dragged every target that is 2 mod 4 back onto the previous instruction,
     * which is how a `b .` looped two bytes early. */
    s.npc = s.thumb ? (target & ~1u) : (target & ~3u);
    s.cycles += CycleModel::kBranch;
    return;
  }

  case AOP_BX: {
    const u32 t = s.regs[d.rm];
    if (d.kind == 2) { raise_undef(s, s.insn); s.cycles += CycleModel::kTrap; return; }  /* BXJ */
    env.stats->branch_taken++;                       /* BX and BLX Rm always jump  */
    const bool to_thumb = (t & 1) != 0;
    if (!to_thumb && !s.thumb && (t & 2) && s.jazelle_trap) {
      raise_undef(s, s.insn);                              /* Jazelle entry    */
      s.cycles += CycleModel::kTrap;
      return;
    }
    if (d.kind == 1 && s.thumb) {
      /* BLX Rm from Thumb state (T1, `0100 0111 1 Rm 000`): the link is the
       * address of the *next* instruction with the T bit set, exactly as for
       * every other Thumb call - the caller is in Thumb state, so bit 0 of the
       * value R14 gets is what sends `bx lr` back into Thumb (A6.6.3).  The
       * ARMv4T "partial link" rule this used to implement - LR = PC+4 plus a
       * slice of the target's high bits - belongs to the old BLX(1) sequence
       * and corrupts R14 here (the oracle's own answer for the case in the
       * corpus is `(pc+2)|1`). */
      const u32 newpc = t & ~1u;
      s.regs[14] = (s.cur + 2u) | 1u;
      s.npc = newpc;
    } else {
      if (d.kind == 1) s.regs[14] = s.cur + 4;             /* BLX from ARM     */
      s.npc = to_thumb ? (t & ~1u) : (t & ~3u);
    }
    s.set_thumb(to_thumb ? 1 : 0);
    s.cpsr = (s.cpsr & ~(CPSR_T | CPSR_J)) | (to_thumb ? u32(CPSR_T) : 0u);
    s.cycles += CycleModel::kBranch;
    return;
  }

  /* -------------------------------------------------- SWI / BKPT ----- */
  case AOP_SWI: raise_swi(s, d.imm24); return;
  case AOP_BKPT: raise_undef(s, s.insn); return;

  /* ------------------------------------------- coprocessor (MCR/MRC) --
   * A register transfer is a privileged operation: from User mode it is
   * undefined, and so is any cp/opc1/alias the core does not implement -
   * the engine asks the environment (IsaEnv::coproc) which is which and
   * answers UNDEF itself.  The ARM926EJ-S implements CP15, its system
   * control coprocessor, through the same callbacks. */
  case AOP_MCR: case AOP_MCR2: {
    if (s.mode() == MODE_USR || !env.coproc_present ||
        !env.coproc_present(env.ctx, d.cp, d.opc1, op == AOP_MCR2)) {
      raise_undef(s, s.insn);
      return;
    }
    /* The value comes from the register file; the core's write callback
     * takes it directly (the ARM926EJ-S's cp15_write used to read regs[rd]
     * itself, from when the engine was a Cpu method). */
    env.coproc_write(env.ctx, d.cp, d.crn, d.opc1, d.crm, d.opc2, s.regs[d.rd]);
    s.cycles += CycleModel::kCoproc;
    return;
  }
  case AOP_MRC: case AOP_MRC2: {
    if (s.mode() == MODE_USR || !env.coproc_present ||
        !env.coproc_present(env.ctx, d.cp, d.opc1, op == AOP_MRC2)) {
      raise_undef(s, s.insn);
      return;
    }
    /* opc1 is forced to 0 on the read: the check above accepts opc1 <= 1,
     * and the ARM926EJ-S CP15 read path keys register c0 off opc1 == 0 -
     * the historical core behaviour read with opc1 = 0 whatever the
     * encoding said, and this engine keeps it bit for bit. */
    const u32 v = env.coproc_read(env.ctx, d.cp, d.crn, 0, d.crm, d.opc2);
    if (d.rd == 15) load_pc(s, v);
    else s.regs[d.rd] = v;
    s.cycles += CycleModel::kCoproc;
    return;
  }

  case AOP_LDC: case AOP_STC:
  case AOP_UNDEF:
  default:
    raise_undef(s, s.insn);
    return;
  }
}

}  // namespace brain::arm::isa::armv5
