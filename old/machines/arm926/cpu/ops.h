/* brain-emu: ARMv5TE datapath primitives shared by the interpreter and the JIT.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Both engines MUST use these so that the flag semantics of the two paths are
 * provably identical (a divergence here is the single most likely source of
 * "the JIT disagrees with the interpreter" bugs).
 *
 * Reference: ARM DDI 0100I chapter B2 (Barrel shifter B2.2.3, conditions
 * B2.2.2, saturating arithmetic B2.2.6).
 */
#ifndef BRAINEMU_CORE_OPS_H
#define BRAINEMU_CORE_OPS_H

#include "machines/arm926/cpu/state.h"

namespace brain::arm926 {

/* shift types (SH_*) and the Shifted result type live in core/state.h */

ALWAYS_INLINE u32 ror32(u32 x, u32 n) {
  n &= 31;
  return n ? ((x >> n) | (x << (32 - n))) : x;
}

/* Barrel shifter with ARM semantics.  `cin` is the incoming C flag.
 *
 * The amount is a *number of bit positions*, already re-encoded for whoever asked:
 * the ARM-state field that would be LSR #0 or ASR #0 means a 32-bit shift and the
 * one that would be ROR #0 is RRX (SH_RRX), and decode_op2_reg() applies that so no
 * caller has to repeat it.  A zero amount otherwise means "no shift", which also
 * leaves C alone - that is what a register-supplied amount of zero does, since the
 * re-encodings belong to the immediate field and not to the shifter (B2.2.3).
 * Right-ward shifts saturate: LSR by 32 or more gives zero with C from bit 31 for
 * exactly 32, ASR by 32 or more gives the sign; a rotate folds as
 * (n - 1) MOD 32 + 1, so ROR #32 is the identity with C from bit 31 and ROR #33 is
 * ROR #1 with C from bit 0.  Reference: ARM DDI 0084D s4.5.2, DDI 0406 A1.8.1.
 */
ALWAYS_INLINE Shifted shift_arm(u32 x, u32 type, u32 amt, u32 cin) {
  if (type == SH_RRX) return Shifted{(cin << 31) | (x >> 1), x & 1};
  if (amt == 0) return Shifted{x, cin};            /* no shift: value and C kept */
  Shifted r{x, cin};
  switch (type) {
    case SH_LSL:
      if (amt < 32) { r.val = x << amt; r.carry = (x >> (32 - amt)) & 1; }
      else { r.val = 0; r.carry = amt == 32 ? (x & 1) : 0; }
      break;
    case SH_LSR:
      if (amt < 32) { r.val = x >> amt; r.carry = (x >> (amt - 1)) & 1; }
      else { r.val = 0; r.carry = amt == 32 ? (x >> 31) & 1 : 0; }
      break;
    case SH_ASR:
      if (amt < 32) {
        r.val = (u32)((s32)x >> amt);
        r.carry = (u32)(((s32)x >> (amt - 1)) & 1);
      } else {
        r.val = ((s32)x < 0) ? 0xFFFFFFFFu : 0;
        r.carry = (x >> 31) & 1;
      }
      break;
    default: {                                     /* SH_ROR */
      const u32 n = ((amt - 1) & 31) + 1;
      r.val = ror32(x, n);
      r.carry = (x >> (n - 1)) & 1;
      break;
    }
  }
  return r;
}

/* ARM's "add three, set carry/overflow" used by ADDS/ADCS and MLS. */
ALWAYS_INLINE void add3_flags(u32 a, u32 b, u32 cin, u32* res, u32* c, u32* v) {
  u64 r = (u64)a + (u64)b + cin;
  u32 lo = (u32)r;
  *res = lo;
  *c = (u32)(r >> 32);
  /* Signed overflow of a + b + cin. */
  u64 s = (u64)(s32)a + (s64)(s32)b + cin;
  *v = ((u32)s != lo) ? 1u : 0u;
}

ALWAYS_INLINE void sub2_flags(u32 a, u32 b, u32 bin, u32* res, u32* c, u32* v) {
  u64 bb = (u64)b + bin;
  u32 lo = (u32)((u64)a - bb);
  *res = lo;
  *c = (a >= (u32)bb) ? 1u : 0u;              /* ARM C == NOT borrow */
  *v = ((s64)(s32)a - (s64)(s32)b - bin) != (s32)lo ? 1u : 0u;
}

/* Saturating helpers (ARMv5E, B2.2.6).  `q` is set when saturation occurs. */
ALWAYS_INLINE u32 saturate32(s64 x, u32* q) {
  if (x > 0x7FFFFFFFLL) { *q = 1; return 0x7FFFFFFFu; }
  if (x < -0x80000000LL) { *q = 1; return 0x80000000u; }
  return (u32)x;
}

ALWAYS_INLINE u32 add_saturate(u32 a, u32 b, u32* q) {
  return saturate32((s64)(s32)a + (s64)(s32)b, q);
}
ALWAYS_INLINE u32 sub_saturate(u32 a, u32 b, u32* q) {
  return saturate32((s64)(s32)a - (s64)(s32)b, q);
}
/* QDADD / QDSUB: second operand is doubled with saturation. */
ALWAYS_INLINE u32 half_sat_double(u32 b, u32* q) {
  s64 doubled = (s64)(s32)b * 2;
  if (doubled > 0x7FFFFFFFLL || doubled < -0x80000000LL) {
    *q = 1;
    doubled = doubled > 0x7FFFFFFFLL ? 0x7FFFFFFF : -0x80000000LL;
  }
  return (u32)doubled;
}

/* Halfword multiply operand selection (B2.2.6.4 / TRM table 3-62). */
ALWAYS_INLINE u32 hw_operand(u32 x, u32 sel) {
  /* sel bit0: 0 low half, 1 high half; applied to a 16-bit signed value. */
  u32 h = (sel & 1) ? (x >> 16) : (x & 0xFFFFu);
  return h;
}
ALWAYS_INLINE s32 hw_signed(u32 x, u32 sel) { return (s32)(s16)hw_operand(x, sel); }
ALWAYS_INLINE u32 hw_unsigned(u32 x, u32 sel) { return hw_operand(x, sel); }

/* 32-bit rotate right used by the "immediate rotated" encoding:
 * value = ROR(imm8, 2*rot), carry = top bit of the result (rot!=0). */
ALWAYS_INLINE Shifted arm_immediate(u32 insn, u32 cin) {
  u32 rot = (insn >> 8) & 0xF;
  u32 imm = insn & 0xFF;
  Shifted s;
  if (rot == 0) { s.val = imm; s.carry = cin; }
  else { s.val = ror32(imm, rot * 2); s.carry = (s.val >> 31) & 1; }
  return s;
}

/* ------------------------------------------------- data-processing ops -- */
enum : u32 {
  DP_AND = 0, DP_EOR = 1, DP_SUB = 2, DP_RSB = 3,
  DP_ADD = 4, DP_ADC = 5, DP_SBC = 6, DP_RSC = 7,
  DP_TST = 8, DP_TEQ = 9, DP_CMP = 10, DP_CMN = 11,
  DP_ORR = 12, DP_MOV = 13, DP_BIC = 14, DP_MVN = 15,
};
ALWAYS_INLINE bool dp_is_logical(u32 dp) {
  return dp == DP_AND || dp == DP_EOR || dp == DP_ORR || dp == DP_MOV ||
         dp == DP_BIC || dp == DP_MVN || dp == DP_TST || dp == DP_TEQ;
}
ALWAYS_INLINE bool dp_flags_only(u32 dp) {
  return dp >= DP_TST && dp <= DP_CMN;
}

/* Flag writes.  Logical operations leave V alone (which is why V is kept in
 * its own Cpu field rather than mirrored in a host OF flag). */
ALWAYS_INLINE void set_logical_flags(Cpu& c, u32 res, u32 carry) {
  c.cpsr = (c.cpsr & ~(CPSR_N | CPSR_Z | CPSR_C)) | (res & CPSR_N) |
           (res == 0 ? u32(CPSR_Z) : 0u) | (carry ? u32(CPSR_C) : 0u);
}
ALWAYS_INLINE void set_arith_flags(Cpu& c, u32 res, u32 carry, u32 ovf) {
  c.cpsr = (c.cpsr & ~CPSR_FLAGS) | (res & CPSR_N) | (res == 0 ? u32(CPSR_Z) : 0u) |
           (carry ? u32(CPSR_C) : 0u) | (ovf ? u32(CPSR_V) : 0u);
  c.vflag = ovf;
}
ALWAYS_INLINE void set_nz_only(Cpu& c, u32 res) {
  c.cpsr = (c.cpsr & ~(CPSR_N | CPSR_Z)) | (res & CPSR_N) | (res == 0 ? u32(CPSR_Z) : 0u);
}

/* 64-bit signed/unsigned multiply helpers (identical in both engines). */
ALWAYS_INLINE u64 mul_arm(u32 a, u32 b, bool us, bool acc, u64 accval) {
  u64 p = us ? (u64)((s64)(s32)a * (s64)(s32)b) : (u64)((u64)a * (u64)b);
  return acc ? p + accval : p;
}

/* -------------------------------------------------------------- cycles --
 * A pipeline-throughput model, deliberately simple and *identical* for the
 * interpreter and the JIT (the JIT charges the same numbers from the same
 * table, so the two paths agree on cycle counts bit for bit):
 *   - every instruction occupies one issue slot;
 *   - each data access adds one cycle (TRM 5.2: a TCM access is a request
 *     cycle plus a data cycle; external/bus-wait timing is SoC level and out
 *     of scope for a CPU-only emulator);
 *   - writing R15 (LDR PC, LDM PC, MOVS PC, ...) restarts the prefetch stage;
 *   - a taken branch restarts the prefetch stage as well;
 *   - shift-by-register needs the ARM9E dynamic shifter;
 *   - 64-bit results (long multiplies) need the extra result stage;
 *   - SWP is a read plus a write; CP15 transfers use the coprocessor bus.
 * The caches are not modelled, so no hit/miss component exists (documented in
 * docs/ISA-COVERAGE.md). */
namespace CycleModel {
enum : u32 {
  kIssue = 1,
  kAccess = 1,
  kPcLoad = 2,
  kBranch = 1,
  kShiftByReg = 1,
  kMulMla = 1,
  kMulLong = 1,
  kSwpExtra = 1,
  kCoproc = 2,
  kTrap = 1,
  kExceptionCycles = 3,
  kUndefCycles = 1,
};
}  // namespace CycleModel

/* Condition -> readable name (trace/log). */
const char* cond_name(u32 c);
const char* mode_name(u32 mode);
const char* exc_name(u32 e);

/* One-line rendering of an instruction, used by --trace on both engines. */
void format_arm_insn(u32 insn, char* out, size_t n);
void format_thumb_insn(u32 insn, char* out, size_t n);

}  // namespace brain::arm926

#endif  /* BRAINEMU_CORE_OPS_H */
