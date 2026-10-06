/* brain-emu: ARM state interpreter (ARMv5TEJ / ARM926EJ-S).
 *
 * Semantics sources, in order of authority for this core:
 *   - ARM DDI 0100I  (the ARM ARM for ARMv4/v5T/v5E/v6) -- reference/DDI01001.pdf
 *   - ARM DDI 0198E  (ARM926EJ-S TRM)                   -- reference/DDI0198E_...pdf
 * Everything the two engines must agree on (barrel shifter, flag updates,
 * saturating arithmetic, unaligned-access permission, cycle charges, block
 * transfer address sequences) goes through core/ops.h, so the interpreter and
 * the JIT cannot drift apart.
 */
#include <cstdio>
#include <cstring>

#include "jit/jit.h"
#include "core/cpu.h"
#include "core/decode.h"
#include "core/ops.h"

namespace brain {

/* ------------------------------------------------------------ operand2 -- */

ALWAYS_INLINE Shifted Cpu::arm_op2(const ArmDec& d) const {
  const u32 cin = (cpsr >> 29) & 1;
  switch (d.op2) {
    case OP2_IMM: {
      /* value = ROR(imm8, 2*rot); carry = rot ? value<31> : C unchanged */
      if (d.rot == 0) return Shifted{d.imm12 & 0xFF, cin};
      const u32 v = ror32(d.imm12 & 0xFF, (d.rot & 0xF) * 2);
      return Shifted{v, (v >> 31) & 1};
    }
    case OP2_REG:
      return Shifted{regs[d.rm], cin};
    case OP2_REG_SHIFT:
      return shift_arm(regs[d.rm], d.sh, d.amt, cin);
    default: {
      /* Shifted by a register: the amount is Rs<7:0> (B2.2.3).  A zero there is a
       * shift by zero - no value change and C left alone - because the #0
       * re-encodings are a property of the immediate field, see decode_op2_reg.
       * Going through shift_arm() is the point: this is the same primitive the JIT
       * front end reads, so the two engines cannot disagree about a shift. */
      const u32 amt = regs[d.rs] & 0xFF;
      return shift_arm(regs[d.rm], d.sh, amt, cin);
    }
  }
}

/* ---------------------------------------------------------- addressing -- */

u32 Cpu::arm_ldst_offset(const ArmDec& d) const {
  if (d.op2 == OP2_NONE) return d.imm12;
  return arm_op2(d).val;
}

/* Block transfer (LDM/STM) address sequence, ARM DDI 0100I A5.4.2..A5.4.5. */
void Cpu::arm_block_addrs(const ArmDec& d, u32 n, u32* start, u32* wb) const {
  const u32 base = regs[d.rn];
  const u32 span = n * 4;
  if (d.u) *start = base + (d.p ? 4 : 0);
  else *start = base - (d.p ? span : span - 4);
  *wb = base + (d.u ? span : 0u - span);
}

/* --------------------------------------------------------- CPSR restore -- */

/* LDR/LDM/MOV into R15 with S set: CPSR <- SPSR (mode switch included). */
ALWAYS_INLINE void Cpu::restore_cpsr_from_spsr() {
  const u32 spsr = bank[bank_idx].spsr;
  const u32 new_mode = spsr & CPSR_MODE_M;
  if (new_mode != (cpsr & CPSR_MODE_M)) {
    /* switch_mode() flushes the current bank and loads the target bank */
    switch_mode(new_mode);
  }
  cpsr = spsr;
  vflag = (spsr >> 28) & 1;
  set_thumb((spsr & CPSR_T) ? 1 : 0);
  priv = (cpsr & CPSR_MODE_M) != MODE_USR;
  sync_cp15_caches();
}

/* A write to R15 that is not a plain branch.  `from_load` marks a load whose
 * result feeds the PC (LDR, LDRH, LDM/POP with R15 in the list): DDI 0198E
 * Table 2-11 gives that case its own rule through CP15 c1[15] (L4) -
 * "0 = loads to PC set the T bit, 1 = loads to PC do not set the T bit (ARMv4
 * behaviour)" - and L4 resets to 0, so an odd load target does change state.
 * Every other writer of the PC (MOV, ORR, ADD, MUL, MRS, MRC) ignores bits[1:0]
 * and cannot change state on this core; ARMv6+ made those behave like BX, so the
 * golden emulator disagrees there and the generator keeps such encodings out of
 * the corpus instead of the core following it.  A PC restored together with an
 * SPSR restore (MOVS-into-PC, LDM-with-S) takes its state from the SPSR and
 * passes from_load = false.  `cur` and R15 are refreshed by the next fetch,
 * which keeps the pipeline rule regs[15] == cur + (thumb ? 4 : 8) true
 * everywhere. */
ALWAYS_INLINE void Cpu::load_pc(u32 value, bool from_load) {
  const bool set_t = from_load && !(cp15.c[C1_CTRL] & SCTLR_L4);
  const bool to_thumb = set_t ? (value & 1u) != 0 : thumb != 0;
  npc = to_thumb ? (value & ~1u) : (value & ~3u);
  if (set_t && to_thumb != (thumb != 0)) {
    set_thumb(to_thumb ? 1u : 0u);
    cpsr = (cpsr & ~(CPSR_T | CPSR_J)) | (to_thumb ? u32(CPSR_T) : 0u);
  }
  if (jit) jit->flush_pending_link();
  cycles += CycleModel::kPcLoad;
}

/* ------------------------------------------------------------- MSR ----- */

void Cpu::apply_msr(u32 mask, u32 val, bool spsr) {
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
    bank[bank_idx].spsr = (bank[bank_idx].spsr & ~allowed) | (val & allowed);
    return;
  }
  if ((val & allowed & CPSR_J) && cfg.jazelle_trap) {
    /* Entering Jazelle state is not implemented (docs/ISA-COVERAGE.md): the
     * attempt takes an Undefined Instruction exception instead. */
    raise_undef(insn);
    return;
  }
  const u32 new_cpsr = (cpsr & ~allowed) | (val & allowed);
  const u32 new_mode = new_cpsr & CPSR_MODE_M;
  if (new_mode != (cpsr & CPSR_MODE_M) && valid_mode(new_mode)) {
    /* switch_mode() banks the old mode's R8-R14/FIQ regs and installs the new
     * mode's bank; the mode field it writes is the same one new_cpsr holds. */
    cpsr = new_cpsr;
    switch_mode(new_mode);
  } else {
    cpsr = new_cpsr;
  }
  vflag = (cpsr >> 28) & 1;
  priv = (cpsr & CPSR_MODE_M) != MODE_USR;
  if (jit) jit->on_env_change("msr");
}

bool Cpu::valid_mode(u32 m) {
  switch (m) {
    case MODE_USR: case MODE_FIQ: case MODE_IRQ: case MODE_SVC:
    case MODE_ABT: case MODE_UND: case MODE_SYS:
      return true;
    default:
      return false;
  }
}

/* ======================================================== exec_arm ===== */

void Cpu::exec_arm(u32 insn_word) {
  insns++;
  stats.insns_interp++;
  cycles += CycleModel::kIssue;

  if (BRAIN_UNLIKELY(!condition_true(insn_word >> 28))) return;  /* NOP: A6.1.1 */

  ArmDec d;
  decode_arm(insn_word, &d);
  exec_arm_dec(d);
}

/* Execute an already-decoded instruction.  Both states share this path: the
 * Thumb decoder fills the same ArmDec, so a Thumb instruction takes exactly
 * the ARM semantics of the equivalent encoding -- which is what the hardware
 * does (A6: most Thumb formats are "the ARM instruction with the same name").
 * The instruction count and the issue cycle are charged by the caller. */
void Cpu::exec_arm_dec(const ArmDec& d) {
  const u32 op = d.op;
  /* `insn` is read from the Cpu: an undefined-instruction report has to quote
   * the word that is stored there, so no local copy is kept. */

  switch (op) {

  /* -------------------------------------------------- data processing -- */
  case AOP_DP: {
    const Shifted s = arm_op2(d);
    const u32 a = regs[d.rn];
    const u32 b = s.val;
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
        const u64 t = (u64)a + (u64)b + (u64)((cpsr >> 29) & 1);
        res = (u32)t;
        nc = (u32)(t >> 32);
        nv = (((~(a ^ b)) & (a ^ res)) >> 31);
        break;
      }
      case DP_SBC: {
        const u32 sub = b + (1 - ((cpsr >> 29) & 1));
        res = a - sub;
        nc = (a >= sub);
        nv = (((a ^ b) & (a ^ res)) >> 31);
        break;
      }
      case DP_RSC: {
        const u32 sub = a + (1 - ((cpsr >> 29) & 1));
        res = b - sub;
        nc = (b >= sub);
        nv = (((b ^ a) & (b ^ res)) >> 31);
        break;
      }
      default: break;
    }

    if (d.s) {
      if (dp_flags_only(d.dp)) {
        if (d.dp == DP_TST || d.dp == DP_TEQ) set_logical_flags(*this, res, s.carry);
        else set_arith_flags(*this, res, nc, nv);
        return;
      }
      if (d.rd == 15) {
        /* "ADDS/MOVS PC, ...": CPSR <- SPSR, ALU flags discarded, PC <- res */
        restore_cpsr_from_spsr();
        load_pc(res);
        return;
      }
      if (dp_is_logical(d.dp)) set_logical_flags(*this, res, s.carry);
      else set_arith_flags(*this, res, nc, nv);
    }
    if (d.rd != 15) regs[d.rd] = res;
    else load_pc(res);
    if (d.op2 == OP2_REG_RS) cycles += CycleModel::kShiftByReg;
    return;
  }

  /* ----------------------------------------------------- MRS / MSR ---- */
  case AOP_MRS: {
    if (d.psr && mode() == MODE_USR) { raise_undef(insn); return; }  /* SPSR in USR */
    const u32 val = d.psr ? bank[bank_idx].spsr : cpsr;
    if (d.rd == 15) load_pc(val);
    else regs[d.rd] = val;
    return;
  }
  case AOP_MSR: {
    if (mode() == MODE_USR) { raise_undef(insn); cycles += CycleModel::kUndefCycles; return; }
    u32 val;
    if (d.op2 == OP2_IMM) val = arm_op2(d).val;
    else val = regs[d.rm];
    apply_msr(d.mask, val, d.psr != 0);
    return;
  }

  /* ------------------------------------------------------- multiply --- */
  case AOP_MUL: {
    u32 res = regs[d.rm] * regs[d.rs];
    if (d.kind) res += regs[d.rn];
    if (d.s) set_nz_only(*this, res);
    if (d.rd == 15) {
      if (cfg.strict_unpredictable) { raise_undef(insn); return; }
      load_pc(res);
    } else {
      regs[d.rd] = res;
    }
    if (d.kind) cycles += CycleModel::kMulMla;
    return;
  }
  case AOP_MULL: {
    const bool us = (d.kind & 2) != 0;      /* signed                    */
    const bool acc = (d.kind & 1) != 0;     /* accumulate (UMLAL/SMLAL)  */
    const u32 lo = d.rd, hi = d.ra;
    if (lo == 15 || hi == 15 || lo == hi) {
      if (cfg.strict_unpredictable) { raise_undef(insn); return; }
    }
    const u64 p = mul_arm(regs[d.rm], regs[d.rs], us, acc,
                          acc ? (((u64)regs[hi] << 32) | regs[lo]) : 0);
    const u32 rlo = (u32)p, rhi = (u32)(p >> 32);
    if (d.s) {
      cpsr = (cpsr & ~(CPSR_N | CPSR_Z)) | (rhi & CPSR_N) |
             ((rlo | rhi) == 0 ? u32(CPSR_Z) : 0u);
    }
    if (lo != 15) regs[lo] = rlo;
    if (hi != 15) regs[hi] = rhi;
    cycles += CycleModel::kMulLong;
    return;
  }
  case AOP_MULH: {
    /* SMLAxy / SMULxy: x (bit 0) selects Rm's half, y (bit 1) selects Rs's.
     * SMULxy leaves Q alone; SMLAxy sets it when the 32-bit accumulate
     * saturates (A3.5.3). */
    const u32 res_half = (u32)((s64)((s32)(s16)hw_operand(regs[d.rm], d.kind & 1)) *
                              (s64)((s32)(s16)hw_operand(regs[d.rs], (d.kind >> 1) & 1)));
    if (d.ra == 0x1F) {                 /* SMULxy: no accumulate, no flags   */
      if (d.rd == 15) load_pc(res_half);
      else regs[d.rd] = res_half;
      return;
    }
    const s64 acc = (s64)(s32)res_half + (s64)(s32)regs[d.ra];
    u32 qq = 0;
    const u32 res = saturate32(acc, &qq);
    if (d.rd == 15) load_pc(res);
    else regs[d.rd] = res;
    set_q(qq != 0);
    return;
  }
  case AOP_MULW: {
    /* SMLAWy / SMULWy: (word x halfword) >> 16 */
    const u32 y = d.kind & 1;
    const bool word_only = (d.kind >> 1) & 1;
    const s64 prod = ((s64)(s32)regs[d.rm] *
                      (s64)((s32)(s16)hw_operand(regs[d.rs], y))) >> 16;
    if (word_only) {
      if (d.rd == 15) load_pc((u32)prod);
      else regs[d.rd] = (u32)prod;
      return;
    }
    u32 qq = 0;
    const u32 res = saturate32(prod + (s64)(s32)regs[d.ra], &qq);
    if (d.rd == 15) load_pc(res);
    else regs[d.rd] = res;
    set_q(qq != 0);
    return;
  }
  case AOP_MULHAL: {
    /* SMLALxy: 32-bit accumulate into RdLo:RdHi, Q set on overflow */
    const s64 prod = (s64)((s32)(s16)hw_operand(regs[d.rm], d.kind & 1)) *
                     (s64)((s32)(s16)hw_operand(regs[d.rs], (d.kind >> 1) & 1));
    /* The pair is assembled in the unsigned domain and reinterpreted: a left
     * shift of a negative s64 is undefined, and the bits are the same. */
    const s64 acc = (s64)((((u64)regs[d.ra]) << 32) | regs[d.rd]);
    const s64 res = prod + acc;
    /* A3.5.3: "SMULxy and SMLALxy do not affect any flags" -- no Q, no NZ. */
    if (d.rd != 15) regs[d.rd] = (u32)(u64)res;
    if (d.ra != 15) regs[d.ra] = (u32)((u64)res >> 32);
    cycles += CycleModel::kMulLong;
    return;
  }

  /* ------------------------------------------------- saturating / CLZ -- */
  case AOP_QD: {
    const u32 a = regs[d.rn], b = regs[d.rm];
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
    if (d.rd == 15) load_pc(res);
    else regs[d.rd] = res;
    set_q(q != 0);
    return;
  }
  case AOP_CLZ: {
    const u32 x = regs[d.rm];
    const u32 res = x ? (u32)__builtin_clz(x) : 32u;
    if (d.rd == 15) load_pc(res);
    else regs[d.rd] = res;
    return;
  }

  /* ------------------------------------------------------------- SWP -- */
  case AOP_SWP: {
    const u32 addr = regs[d.rn];
    /* B2.6.2: SWP and SWPB must always be word aligned. */
    if (addr & 3) { raise_dabort(FS926_ALIGN, addr); return; }
    const u32 sz = d.b ? 1u : 4u;
    const u32 v = mem_read(addr, sz, (d.b ? MF_SZ8 : MF_SZ32) | MF_SWP);
    if (BRAIN_UNLIKELY(exectrap)) return;
    mem_write(addr, regs[d.rm], sz, (d.b ? MF_SZ8 : MF_SZ32) | MF_SWP | MF_WRITE);
    if (BRAIN_UNLIKELY(exectrap)) return;
    if (d.rd == 15) load_pc(v);
    else regs[d.rd] = v;
    cycles += CycleModel::kSwpExtra;
    return;
  }

  /* ------------------------------------------------- LDR / STR (32/8) -- */
  case AOP_LDRST: {
    const u32 off = arm_ldst_offset(d);
    /* A5.3.1/A6.5: with R15 as the base the address is the PC value read by
     * this instruction with bits[1:0] treated as zero -- which matters only in
     * Thumb state, where the instruction itself may be halfword aligned. */
    const u32 base = d.rn == 15 ? (regs[15] & ~3u) : regs[d.rn];
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
      const u32 v = mem_read(ea, sz, flags);
      if (BRAIN_UNLIKELY(exectrap)) return;      /* abort: Rn not updated */
      if (d.rd == 15) load_pc(v, /*from_load=*/true);
      else regs[d.rd] = v;
    } else {
      /* STR R15 stores the instruction address + 8, i.e. the pipeline value. */
      const u32 src = d.rd;
      const u32 data = (src == 15) ? regs[15] : regs[src];
      mem_write(ea, data, sz, flags | MF_WRITE);
      if (BRAIN_UNLIKELY(exectrap)) return;
    }
    if (wb) {
      /* "LDR Rn,[Rn,#x]!" leaves Rn with the loaded value (UNPREDICTABLE;
       * the ARM926EJ-S keeps the loaded value -- match that).            */
      if (!(d.l && d.rn == d.rd)) regs[d.rn] = next;
    }
    cycles += CycleModel::kAccess;
    return;
  }

  /* --------------------------------- LDRH/STRH/LDRSB/LDRSH (extra LDST) -- */
  case AOP_LDRSTH: {
    const u32 off = arm_ldst_offset(d);
    const u32 base = d.rn == 15 ? (regs[15] & ~3u) : regs[d.rn];
    const bool wb = (d.am == AM_IMM_PRE_WB || d.am == AM_REG_PRE_WB ||
                     d.am == AM_IMM_POST || d.am == AM_REG_POST);
    const u32 ea = (d.am == AM_IMM_POST || d.am == AM_REG_POST) ? base
                                                                : (d.u ? base + off : base - off);
    const u32 next = d.u ? base + off : base - off;
    /* H=1 -> 16 bit, H=0 with S=1 -> 8 bit (signed byte) */
    const u32 sz = d.h ? 2u : 1u;
    const u32 flags = (sz == 2 ? MF_SZ16 : MF_SZ8) | (sz == 1 ? u32(MF_UNAL) : 0u);
    if (d.l) {
      u32 v = mem_read(ea, sz, flags);
      if (BRAIN_UNLIKELY(exectrap)) return;
      if (d.signed_) v = sz == 2 ? (u32)(s32)(s16)v : (u32)(s32)(s8)v;
      if (d.rd == 15) load_pc(v, /*from_load=*/true);
      else regs[d.rd] = v;
    } else {
      /* A5.3.3: in this encoding space a store names its source in bits[15:12]
       * (the Rd field) and only the halfword form exists. */
      mem_write(ea, regs[d.rd], 2, MF_SZ16 | MF_WRITE);
      if (BRAIN_UNLIKELY(exectrap)) return;
    }
    if (wb && !(d.l && d.rn == d.rd)) regs[d.rn] = next;
    cycles += CycleModel::kAccess;
    return;
  }

  /* ----------------------------------------------------- LDRD / STRD -- */
  case AOP_LDRD: {
    const u32 off = arm_ldst_offset(d);
    const u32 base = d.rn == 15 ? (regs[15] & ~3u) : regs[d.rn];
    const bool wb = (d.am == AM_IMM_PRE_WB || d.am == AM_REG_PRE_WB ||
                     d.am == AM_IMM_POST || d.am == AM_REG_POST);
    const u32 ea = (d.am == AM_IMM_POST || d.am == AM_REG_POST) ? base
                                                                : (d.u ? base + off : base - off);
    const u32 next = d.u ? base + off : base - off;
    if (d.rd == 15 || (d.rd & 1)) { raise_undef(insn); return; }
    const u32 flags = MF_SZ32;
    if (!d.kind) {
      const u32 v0 = mem_read(ea, 4, flags);
      if (BRAIN_UNLIKELY(exectrap)) return;
      const u32 v1 = mem_read(ea + 4, 4, flags);
      if (BRAIN_UNLIKELY(exectrap)) return;
      regs[d.rd] = v0;
      regs[d.rd + 1] = v1;
    } else {
      mem_write(ea, regs[d.rd], 4, flags | MF_WRITE);
      if (BRAIN_UNLIKELY(exectrap)) return;
      mem_write(ea + 4, regs[d.rd + 1], 4, flags | MF_WRITE);
      if (BRAIN_UNLIKELY(exectrap)) return;
    }
    if (wb) regs[d.rn] = next;
    cycles += CycleModel::kAccess;
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
    arm_block_addrs(d, n, &start, &wbv);

    /* User-bank transfer (A5.4: S=1 always for LDM with PC? no: for LDM, S=1
     * with PC means "restore CPSR from SPSR"; S=1 without PC means user bank.
     * For STM, S=1 always transfers the user bank; and STM with R15 in the
     * list from a privileged mode always stores the user R15. */
    bool user_bank;
    bool restore_flags = false;
    if (load) {
      if (d.kind && mode() == MODE_USR) { raise_undef(insn); return; }
      if (list & (1u << 15)) restore_flags = d.kind != 0;
      user_bank = (d.kind != 0) && !(list & (1u << 15));
    } else {
      user_bank = d.kind != 0;
      if (user_bank && mode() == MODE_USR) { raise_undef(insn); return; }
    }
    const u32 saved_bank = bank_idx;
    if (user_bank) { flush_bank(); load_bank(BANK_USR); }

    u32 addr = start;
    for (u32 i = 0; i < 16; ++i) {
      if (!(list & (1u << i))) continue;
      if (load) {
        const u32 v = mem_read(addr, 4, MF_SZ32);
        if (BRAIN_UNLIKELY(exectrap)) {
          if (user_bank) load_bank(saved_bank);
          return;                        /* abort: Rn is not written back   */
        }
        regs[i] = v;
      } else {
        /* STM of R15 always stores the *pipeline* value of R15 (i.e. +12
         * here) except in the user-bank case, where the user R15 is stored. */
        const u32 v = (i == 15 && !user_bank) ? regs[15] : regs[i];
        mem_write(addr, v, 4, MF_SZ32 | MF_WRITE);
        if (BRAIN_UNLIKELY(exectrap)) {
          if (user_bank) {
            bank[BANK_USR].r[15] = regs[15];
            load_bank(saved_bank);
          }
          return;
        }
      }
      addr += 4;
    }
    if (user_bank) {
      if (load) bank[BANK_USR].r[15] = regs[15];
      load_bank(saved_bank);
    }
    if (d.w) regs[d.rn] = wbv;
    if (load && (list & (1u << 15))) {
      const u32 newpc = regs[15];
      if (restore_flags) restore_cpsr_from_spsr();
      load_pc(newpc, /*from_load=*/!restore_flags);
    }
    cycles += CycleModel::kAccess * (n - 1);
    return;
  }

  /* --------------------------------------------------- branches ------ */
  case AOP_B: {
    /* Thumb state: the long branch is a *pair of independent halfwords* in
     * ARMv5T (A7.1.17), so each half is its own instruction here.  kind 4 is
     * the H==10 setup half, 5 the H==11 (BL) half and 6 the H==01 (BLX to ARM)
     * half.  They work off LR, not off the PC, which is what lets an
     * assembler place them anywhere in a ±4MB window. */
    if (d.kind >= 4) {
      if (d.kind == 4) { regs[14] = regs[15] + d.imm24; return; }
      const u32 target = regs[14] + d.imm24;
      regs[14] = (cur + 2) | 1u;
      if (d.kind == 6) {                       /* BLX: switch to ARM state     */
        npc = target & ~3u;
        set_thumb(0);
        cpsr &= ~(CPSR_T | CPSR_J);
        cycles += CycleModel::kBranch + CycleModel::kPcLoad;
        if (jit) jit->flush_pending_link();
        return;
      }
      npc = target & ~1u;
      cycles += CycleModel::kBranch;
      if (jit) jit->flush_pending_link();
      return;
    }
    /* Branches read the PC as `cur + 8` (ARM state), which is exactly what
     * regs[15] holds, and LR = cur + 4 for BL / cur & ~3 for BLX. */
    const u32 target = regs[15] + d.imm24;
    if (d.kind == 1) regs[14] = cur + 4;                    /* BL        */
    /* kind 3 = Thumb B<cond>, already filtered by Cpu::exec_thumb. */
    if (d.kind == 2) {                                       /* BLX #imm */
      /* The link carries the state to return *to*.  The caller is in ARM state,
       * so bit[0] stays clear: `BX R14`, or `POP {PC}` with L4 = 0 (where a load
       * takes its state from bit[0]), then lands back in ARM.  Setting bit[0]
       * here would send the return straight back into the Thumb callee. */
      regs[14] = cur + 4;
      set_thumb(1);
      cpsr |= CPSR_T;
      cpsr &= ~CPSR_J;
      /* The target already carries bit[1] from H, so only bit[0] is dropped -
       * masking to a word boundary would discard the half that selects between
       * two adjacent Thumb instructions. */
      npc = target & ~1u;
      cycles += CycleModel::kBranch + CycleModel::kPcLoad;
      if (jit) jit->flush_pending_link();
      return;
    }
    /* A Thumb branch target is only halfword aligned: applying the ARM rule here
     * dragged every target that is 2 mod 4 back onto the previous instruction,
     * which is how a `b .` looped two bytes early. */
    npc = thumb ? (target & ~1u) : (target & ~3u);
    cycles += CycleModel::kBranch;
    if (jit) jit->flush_pending_link();
    return;
  }

  case AOP_BX: {
    const u32 t = regs[d.rm];
    if (d.kind == 2) { raise_undef(insn); cycles += CycleModel::kTrap; return; }  /* BXJ */
    const bool to_thumb = (t & 1) != 0;
    if (!to_thumb && !thumb && (t & 2) && cfg.jazelle_trap) {
      raise_undef(insn);                              /* Jazelle entry    */
      cycles += CycleModel::kTrap;
      return;
    }
    if (d.kind == 1 && thumb) {
      /* BLX Rm from Thumb state: partial link (A4.1.9/A6.1) */
      const u32 newpc = t & ~1u;
      const u32 rot1 = (t >> 1) | (t << 31);
      regs[14] = regs[15] + (rot1 & 0xFF800000u);
      npc = newpc;
    } else {
      if (d.kind == 1) regs[14] = cur + 4;             /* BLX from ARM     */
      npc = to_thumb ? (t & ~1u) : (t & ~3u);
    }
    set_thumb(to_thumb ? 1 : 0);
    cpsr = (cpsr & ~(CPSR_T | CPSR_J)) | (to_thumb ? u32(CPSR_T) : 0u);
    cycles += CycleModel::kBranch;
    if (jit) jit->flush_pending_link();   /* the link target just changed  */
    return;
  }

  /* -------------------------------------------------- SWI / BKPT ----- */
  case AOP_SWI: raise_swi(d.imm24); return;
  case AOP_BKPT: raise_undef(insn); return;

  /* ------------------------------------------------------ CP15 ------- */
  case AOP_MCR: case AOP_MCR2: {
    if (d.cp != 15 || d.opc1 > 1 || op == AOP_MCR2 || mode() == MODE_USR) {
      raise_undef(insn);
      return;
    }
    /* cp15_write takes the *number* of the source register: it reads the
     * value itself, so passing regs[d.rd] here would index with a value. */
    cp15_write(d.crn, d.opc1, d.rd, d.crm, d.opc2);
    cycles += CycleModel::kCoproc;
    return;
  }
  case AOP_MRC: case AOP_MRC2: {
    if (d.cp != 15 || d.opc1 > 1 || op == AOP_MRC2 || mode() == MODE_USR) {
      raise_undef(insn);
      return;
    }
    const u32 v = cp15_read(d.crn, 0, d.crm, d.opc2);
    if (d.rd == 15) load_pc(v);
    else regs[d.rd] = v;
    cycles += CycleModel::kCoproc;
    return;
  }

  case AOP_LDC: case AOP_STC:
  case AOP_UNDEF:
  default:
    raise_undef(insn);
    return;
  }
}

/* ================================================= fetch_and_execute ==== */

/* Instruction address of the *next* step relative to the executing one. */
ALWAYS_INLINE u32 Cpu::insn_size() const { return thumb ? 2u : 4u; }
ALWAYS_INLINE u32 Cpu::pc_bias() const { return thumb ? 4u : 8u; }

ALWAYS_INLINE u32 Cpu::fetch16(GAddr a) {
  if (BRAIN_LIKELY(fetch_e != nullptr && fetch_e->host)) {
    const u8* p = fetch_e->host + (a & 0xFFF);
    return (u32)p[0] | ((u32)p[1] << 8);
  }
  return mem_read(a, 2, MF_SZ16 | MF_FETCH);
}
ALWAYS_INLINE u32 Cpu::fetch32(GAddr a) {
  if (BRAIN_LIKELY(fetch_e != nullptr && fetch_e->host)) {
    u32 out;
    memcpy(&out, fetch_e->host + (a & 0xFFF), 4);
    return out;
  }
  return mem_read(a, 4, MF_SZ32 | MF_FETCH);
}

void Cpu::fetch_and_execute() {
  /* Prime the pipeline: `cur` becomes the instruction we are about to execute
   * and R15 gets the value that instruction will read (cur + 8 / cur + 4).
   * A prefetch abort is reported against `cur` before it is overwritten. */
  cur = npc;
  /* A fetch that aborts retires no instruction, but the step still has to
   * charge one instruction and one cycle: the abort has been taken inside
   * prepare_fetch(), the pipeline has moved to the vector, and a bounded run
   * (either insn- or cycle-budgeted) must be able to terminate even when the
   * vector itself faults. */
  if (BRAIN_UNLIKELY(!prepare_fetch())) { insns++; cycles++; return; }

  if (thumb) {
    const u32 hw = fetch16(cur);
    if (BRAIN_UNLIKELY(exectrap)) {
      take_exception(EXC_PABORT);
      insns++;
      cycles++;
      return;
    }
    insn = hw;
    npc = cur + 2;
    regs[15] = cur + 4;
    exec_thumb(hw);
    return;
  }

  const u32 w = fetch32(cur);
  if (BRAIN_UNLIKELY(exectrap)) {
    take_exception(EXC_PABORT);
    insns++;
    cycles++;
    return;
  }
  insn = w;
  npc = cur + 4;
  regs[15] = cur + 8;
  exec_arm(w);
}

}  // namespace brain
