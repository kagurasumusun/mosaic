/* x86-64 code generation for the JIT's IR.  See jit/emit_x64.h for the register
 * plan and why the flags are split across RFLAGS (N, Z, C) and a register (V).
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Every instruction goes through the same steps:
 *
 *   1. load the guest registers it reads and pin them, so nothing can evict an
 *      operand while the instruction's own scratch registers are handed out;
 *   2. put operand 2 in place - an immediate is folded into the operation, a
 *      shifted register is materialised into the scratch register, anything else
 *      is already where it belongs;
 *   3. pick the destination: the register the guest register already lives in,
 *      a pool register for a flag-only form (its value is dead, its flags are
 *      not), and otherwise a fresh one - evicting by Belady when the pool is
 *      full, never evicting a pinned operand;
 *   4. emit the operation, then whatever fixup ARM's flag rules need that x86
 *      does not give for free.
 *
 * Step 4 has three shapes, decided by the liveness pass in run():
 *   - flags dead     : the plain operation and no fixup at all.  An `ands` whose
 *                      flags nothing reads is just an `and`;
 *   - flags written  : N, Z and C are already what x86's operation produces, so
 *                      only V needs an instruction (`seto`);
 *   - flags preserved: an operation that must not disturb them.  Where the
 *                      sequence is flag-free (`mov`, `lea`, `not`) that costs
 *                      nothing; otherwise a pushfq/popfq pair covers it.
 *
 * The one rule with no x86 instruction behind it is ARM's "a flag-setting
 * logical operation with an unshifted operand leaves C alone": `and`/`or`/`xor`
 * always write CF.  That case stashes the old carry in the scratch register
 * (`setcc` before, `bt` after) - both instructions are flag-free, so they cannot
 * corrupt the very flags they are repairing.
 */
#include "jit/emit_x64.h"

#include <cstddef>
#include <cstring>

#include "core/state.h"   /* SH_* : the shift-type numbering the IR uses */
#include "jit/abi.h"
#include "jit/x64_asm.h"

namespace brain {

/* Per-op acceptance, consulted by the front end before it appends an
 * instruction to a block (see jit/frontend.cpp). */
bool jit_x64_accepts(const IrInsn& in) {
  switch (in.op) {
    case IR_MOV: case IR_MOVI: case IR_ADD: case IR_SUB: case IR_RSB:
    case IR_ADC: case IR_SBC: case IR_AND: case IR_ORR: case IR_EOR:
    case IR_BIC: case IR_MVN: case IR_NEG: case IR_RRX:
      break;
    case IR_LSL:
      if (in.shift_amt > 31) return false;
      break;
    case IR_LSR: case IR_ASR: case IR_ROR:
      /* A zero amount in these encodings means a 32-bit shift, which a
       * five-bit host count field cannot express.  The front end refuses them;
       * this is the second line of defence. */
      if (in.shift_amt == 0 || in.shift_amt > 31) return false;
      break;
    default:
      return false;
  }
  if (in.op == IR_ADC || in.op == IR_SBC) {
    /* A shifted operand would make ARMv5 take the carry input out of the
     * shifter rather than out of CPSR (the pre-ARMv6 ADC quirk), which the
     * front end refuses; the emitter reads the host CF directly. */
    if (in.shift_amt != 0) return false;
  }
  return true;
}

namespace {

using namespace x64;

/* ------------------------------------------------------------------ plan -- */

constexpr int kPool[] = {RAX, RCX, RDX, RSI, RDI, R8, R9, R10, RBX, R12, R13, R14};
constexpr int kPoolCount = (int)(sizeof(kPool) / sizeof(kPool[0]));
constexpr int kScratch = R11;   /* materialised operand / carry stash  */
constexpr int kVreg = R15;      /* the guest's V, 0/1 in r15d          */
constexpr int kShadow = RBP;    /* the shadow pointer                  */
constexpr int kGuestRegs = 13;  /* R0-R12 are the ones a block may name */
constexpr int kMaxPins = 3;

bool is_callee_saved_pool(int r) {
  return r == RBX || r == R12 || r == R13 || r == R14;
}

/* ----------------------------------------------------------- IR analysis -- */

struct Shape {
  bool reads_a;    /* the op reads R(a)                                  */
  bool reads_b;    /* ...and R(b), its second operand                     */
  bool mov_like;   /* dst = f(shifted R(a)) - the MOV/MVN/shift family   */
  bool logical;    /* C comes from the shifter, V is left alone          */
  bool arith;      /* N, Z, C and V all come out of the result           */
  bool mem;        /* a load or store (see jit/ir.h)                     */
};

Shape shape_of(const IrInsn& in) {
  Shape s{};
  switch (in.op) {
    case IR_MOV: case IR_LSL: case IR_LSR: case IR_ASR: case IR_ROR:
    case IR_RRX: case IR_MVN:
      s.reads_a = true; s.mov_like = true; s.logical = true;
      break;
    case IR_MOVI:
      s.logical = true;
      break;
    case IR_NEG:
      s.reads_a = true; s.arith = true;
      break;
    case IR_AND: case IR_ORR: case IR_EOR: case IR_BIC:
      s.reads_a = true; s.reads_b = true; s.logical = true;
      break;
    case IR_ADD: case IR_SUB: case IR_RSB: case IR_ADC: case IR_SBC:
      s.reads_a = true; s.reads_b = true; s.arith = true;
      break;
    default:
      break;
  }
  return s;
}

/* The carry is an *input* to these beyond being a flag they write: an ADC/SBC
 * adds it into the result and RRX shifts it in, so they need it correct even
 * when nothing observes the flags they produce.  An RRX *operand* counts too,
 * whatever the operation is - `add r0, r1, r2, ror #0` is an addition that
 * still reads the carry, and the host `rcr` gets it from the flag register. */
bool reads_carry_for_value(const IrInsn& in) {
  return in.op == IR_ADC || in.op == IR_SBC || in.op == IR_RRX ||
         (!in.b_is_imm && in.shift_type == SH_RRX);
}

/* Operand 2 needs no barrel shift: a plain register, or an immediate whose
 * rotation supplies no carry.  RRX is the exception that makes this worth
 * having as a function: it is `ROR #0`, which the front end encodes as
 * shift_type = SH_RRX with a zero amount, so a bare amount test would call it
 * "no shift" and use the register unrotated. */
bool operand_unshifted(const IrInsn& in) {
  if (in.b_is_imm) return in.imm_rot == 0;
  if (in.shift_type == SH_RRX) return false;
  return in.shift_amt == 0;
}

/* The bit of the *unmodified* source operand ARM's shifter carry comes from:
 * a left shift drops bit 32-n, every right-ward operation drops bit n-1, and
 * RRX drops bit 0.  Mirrors frontend.cpp's shifter_carry_bit(). */
int shifter_carry_bit(uint32_t type, uint32_t amt) {
  if (amt == 0 || type == SH_RRX) return 0;
  if (type == SH_LSL) return (int)((32 - amt) & 31);
  return (int)amt - 1;
}

/* ------------------------------------------------------------- liveness --- */

struct Liveness {
  bool live_in;
  bool live_out;
  /* V is not part of the host flag register on this backend: it lives in a
   * register (r15) that `seto` writes and everything else leaves alone.  That
   * makes it a *separate* live variable from N, Z and C: an `ands` preserves
   * the architecturally correct V by not touching r15, so the V it hands to the
   * block's exit is whatever the last arithmetic instruction put there - which
   * an arithmetic instruction whose own flags nobody reads would otherwise
   * never have computed.  `v_live_out` is "V leaving this instruction is
   * observed", and it is what decides whether that `seto` is still needed. */
  bool v_live_out;
};

/* The flags as one live variable over a branch-free block.  `live_out` is "the
 * flags after this instruction must be the architectural ones": a block is
 * entered and left through the CPSR word, so the last writer's flags are live.
 * When nothing in the block writes them, the CPSR's flag bits are left alone
 * entirely - which is what makes a chain of `and`/`orr` free of any flag
 * bookkeeping. */
/* A flag-setting form whose C output *is* its C input: every logical operation
 * with an unshifted operand, which on ARM means "C is left alone" (MOV and
 * LSL #0 included, and a rotated immediate, whose rotate supplies a carry, is
 * *not* one of these).  Such an instruction reads the flags it writes, so the
 * entry carry has to be real whenever its result is observed. */
bool copies_carry_through(const IrInsn& in) {
  if (!in.setflags) return false;
  const Shape sh = shape_of(in);
  return sh.logical && operand_unshifted(in);
}

void compute_liveness(const IrBlock& b, Liveness* lv) {
  /* The exit materialises the flags whenever *any* instruction wrote them: the
   * value that has to survive to the end of the block is the last writer's, and
   * nothing outside the block can see it otherwise.  When none does, the CPSR's
   * flag bits are the entry ones and are simply left alone. */
  bool live = false;
  for (uint32_t i = 0; i < b.n; ++i)
    if (b.in[i].setflags) live = true;
  /* The exit stores V whenever it stores any flag bit. */
  bool v = live;
  for (int i = (int)b.n - 1; i >= 0; --i) {
    const IrInsn& in = b.in[i];
    const bool def = in.setflags != 0;
    /* Live going in: something after needs the flags (if this instruction does
     * not write them, or writes them as a copy of the old ones), or this
     * instruction needs the carry for its own result. */
    lv[i].live_out = live;
    live = live && (!def || copies_carry_through(in)) || reads_carry_for_value(in);
    lv[i].live_in = live;
    /* V again, from the same point of view: an arithmetic op defines V (one
     * `seto`, which the emitter will emit whenever either variable says the
     * value is wanted), and every other op - including a flag-setting logical,
     * which ARM leaves V alone for - passes the register through. */
    lv[i].v_live_out = v;
    const Shape sh = shape_of(in);
    if (def && sh.arith) v = false;
  }
}

/* =============================================================== emitter == */

class Emitter {
 public:
  explicit Emitter(const IrBlock& blk) : blk_(blk) {}
  bool run(std::vector<uint8_t>* out, JitBlockMeta* meta);

 private:
  void reset();
  void spill(int g);
  void evict(const int* pins, int npins, int at);
  int take(const int* pins, int npins, int at);
  int need(int g, const int* pins, int npins, int at, bool for_write);
  int temp(int at, const int* pins, int npins);
  void release_temp(int r);
  int alloc_dst(const IrInsn& ins, const Shape& sh, const Liveness& lv, int idx,
                const int* pins, int npins, int carry_reg, bool& ok);

  bool op(int idx, const Liveness& lv, const Shape& sh);
  void prologue();
  void epilogue();

  const IrInsn& in(int i) const { return blk_.in[i]; }

  const IrBlock& blk_;
  Buf body_;
  int host_of_[kGuestRegs];
  bool dirty_[kGuestRegs];
  uint8_t use_[IR_MAX_INSNS + 2][kGuestRegs];
  bool taken_[16];
  bool ever_[16];
  bool entry_flags_ = false;
  bool exit_flags_ = false;
  uint32_t end_pc_ = 0;
};

void Emitter::reset() {
  for (int g = 0; g < kGuestRegs; ++g) {
    host_of_[g] = -1;
    dirty_[g] = false;
  }
  for (int r = 0; r < 16; ++r) {
    taken_[r] = false;
    ever_[r] = false;
  }
}

/* ---- allocation --------------------------------------------------------- */

void Emitter::spill(int g) {
  const int r = host_of_[g];
  if (r < 0) return;
  if (dirty_[g]) body_.mov_mem_r(Mem::bx(kShadow, SH_R0 + 4 * g), r);
  host_of_[g] = -1;
  dirty_[g] = false;
  taken_[r] = false;
}

/* Belady over straight-line code: drop the resident guest register whose next
 * use is furthest away, preferring one with no store pending.  Pinned registers
 * (the current instruction's operands) are never touched. */
void Emitter::evict(const int* pins, int npins, int at) {
  int best = -1;
  uint32_t best_use = 0;
  bool best_clean = false;
  for (int g = 0; g < kGuestRegs; ++g) {
    if (host_of_[g] < 0) continue;
    bool pin = false;
    for (int i = 0; i < npins; ++i)
      if (pins[i] == g) pin = true;
    if (pin) continue;
    const uint32_t u = use_[at + 1][g];
    const uint32_t far = u ? u : 0xFFFFFFFFu;
    const bool clean = !dirty_[g];
    if (best < 0 || far > best_use || (far == best_use && clean && !best_clean)) {
      best = g; best_use = far; best_clean = clean;
    }
  }
  if (best >= 0) spill(best);
}

int Emitter::take(const int* pins, int npins, int at) {
  for (int i = 0; i < kPoolCount; ++i) {
    if (taken_[kPool[i]]) continue;
    taken_[kPool[i]] = true;
    ever_[kPool[i]] = true;
    return kPool[i];
  }
  evict(pins, npins, at);
  for (int i = 0; i < kPoolCount; ++i) {
    if (taken_[kPool[i]]) continue;
    taken_[kPool[i]] = true;
    ever_[kPool[i]] = true;
    return kPool[i];
  }
  return -1;                        /* pool exhausted: the block is refused */
}

int Emitter::need(int g, const int* pins, int npins, int at, bool for_write) {
  if (host_of_[g] >= 0) return host_of_[g];
  const int r = take(pins, npins, at);
  if (r < 0) return -1;
  host_of_[g] = r;
  dirty_[g] = false;
  if (!for_write) body_.mov_rm(r, Mem::bx(kShadow, SH_R0 + 4 * g));
  return r;
}

/* A scratch register for a flag-only form (TST/CMP/CMN: the destination is
 * dead, but the operation still needs somewhere to put the result).  The pins
 * are the same as the operation's operands, for the same reason as in take(). */
int Emitter::temp(int at, const int* pins, int npins) {
  return take(pins, npins, at);
}

void Emitter::release_temp(int r) {
  if (r >= 0 && r != kScratch) taken_[r] = false;
}

/* --------------------------------------------------------------- one op -- */

bool Emitter::op(int idx, const Liveness& lv, const Shape& sh) {
  const IrInsn& ins = in(idx);
  const bool def = ins.setflags != 0;
  const bool want = def && lv.live_out;   /* materialise the flags after  */
  const bool pres = !def && lv.live_in;   /* must not disturb the flags   */
  const bool no_dst = ins.dst == IR_NO_DST;

  /* A flag-only op nobody reads is free - unless its V is passed on to
   * something that does read that (see Liveness::v_live_out). */
  if (no_dst && !lv.live_out && !lv.v_live_out) return true;

  /* --- operands --------------------------------------------------------- */
  const bool imm_op = sh.reads_b && ins.b_is_imm && ins.op != IR_MOVI;
  /* MVN is spelled like the MOV family - its operand was moved into `a` by the
   * front end - so an immediate one has *no* register operand at all: the value
   * is `imm` and `a` is whatever the low nibble of the immediate field happened
   * to encode, which is a guest register but not one this instruction reads. */
  const bool mvn_imm = ins.op == IR_MVN && ins.b_is_imm;
  const bool reads_a = sh.reads_a && ins.op != IR_MOVI && !mvn_imm;

  int pins[kMaxPins];
  int npins = 0;
  if (reads_a) pins[npins++] = (int)ins.a;
  if (sh.reads_b && !imm_op) pins[npins++] = (int)ins.b;

  const int ga = reads_a ? need((int)ins.a, pins, npins, idx, false) : -1;
  const int gb = (sh.reads_b && !imm_op) ? need((int)ins.b, pins, npins, idx, false) : -1;
  if ((reads_a && ga < 0) || (sh.reads_b && !imm_op && gb < 0)) return false;

  /* The barrel-shifted *operand* (as opposed to a shift that is the operation
   * itself, which is what the MOV/LSL/LSR/ASR/ROR/RRX family is).  Every
   * two-operand form has one - including ADD and its relatives, whose carry
   * then still comes from the adder rather than from the shifter (see the
   * AArch64 emitter's note on the same rule). */
  const bool op_shift =
      !mvn_imm && (sh.reads_b || ins.op == IR_MVN) && !operand_unshifted(ins);

  /* The carry of a flag-setting logical operation comes from a bit of the
   * unmodified source operand when there is a shift, and from the old C when
   * there is not (the one case x86 cannot express at all). */
  const bool logical_writes_c = want && sh.logical;
  const bool carry_bit = logical_writes_c && (op_shift || (sh.mov_like && ins.shift_amt != 0));
  const bool carry_const = logical_writes_c && ins.b_is_imm && ins.imm_rot != 0;
  const bool carry_keep = logical_writes_c && !carry_bit && !carry_const;
  const int carry_reg = mvn_imm ? -1 : (ins.op == IR_MVN) ? ga : gb;

  /* Does the emitted sequence clobber the flags, and can it be made not to? */
  bool op_flag_free = false;
  switch (ins.op) {
    case IR_MOV: case IR_MOVI: case IR_MVN: op_flag_free = true; break;
    case IR_ADD: op_flag_free = !want; break;                   /* lea */
    case IR_SUB: op_flag_free = !want && imm_op; break;         /* lea */
    case IR_LSL: op_flag_free = ins.shift_amt == 0; break;
    default: break;
  }
  const bool shift_flag_free =
      !op_shift || (ins.shift_type == SH_LSL && ins.shift_amt >= 1 && ins.shift_amt <= 3);

  /* --- destination ------------------------------------------------------ */
  int dv;
  if (no_dst) {
    dv = temp(idx, pins, npins);
  } else if (host_of_[ins.dst] >= 0) {
    dv = host_of_[ins.dst];
  } else {
    dv = take(pins, npins, idx);
    if (dv >= 0) host_of_[ins.dst] = dv;
  }
  if (dv < 0) return false;

  /* A carry fixup reads the source *after* the operation has run, so the
   * destination must not be that register: `ands rd, rn, rm, lsl #2` with
   * rd == rm is the case (rd is both operand and destination).  The register
   * the destination just left keeps the operand alive until the end of the
   * instruction, so it is held rather than freed here - the materialised shift
   * below would otherwise be handed the very register it has to read, and a
   * no-op `mov` there would rotate the operand out from under the fixup. */
  int hold_reg = -1;
  if (carry_bit && !no_dst && dv == carry_reg) {
    const int old = dv;
    host_of_[ins.dst] = -1;
    dirty_[ins.dst] = false;
    const int nd = take(pins, npins, idx);      /* `old` is still marked taken */
    if (nd < 0) return false;
    dv = nd;
    host_of_[ins.dst] = dv;
    hold_reg = old;
  }
  if (!no_dst) dirty_[ins.dst] = true;

  /* --- flag protection -------------------------------------------------- */
  /* A preserved flag set has to survive this instruction.  The two candidates
   * for clobbering it are the materialised shift (below) and the operation
   * itself, and each is wrapped with pushfq/popfq only when it actually writes
   * a flag - so the common shapes (`add`, `and` with an immediate) stay two
   * instructions. */
  const bool shift_clobbers = op_shift && !shift_flag_free;
  const bool op_clobbers = !op_flag_free;
  const bool ps_shift = pres && shift_clobbers;   /* open before the shift  */
  const bool pe_shift = ps_shift && !op_clobbers; /* ...and close after it  */
  const bool ps_op = pres && op_clobbers && !shift_clobbers;
  const bool pe_op = pres && op_clobbers;
  if (ps_shift) body_.pushfq();

  /* --- operand 2 -------------------------------------------------------- */
  int op2 = imm_op ? -1 : gb;
  int op2_temp = -1;
  if (op_shift && !imm_op) {
    /* LSL #1..#3 is one flag-free `lea`; everything else is a host shift, which
     * clobbers CF and OF - harmless while the flags are dead or being written,
     * and the wrapper below covers the case where they are not. */
    const bool shift_reg_is_a = (ins.op == IR_MVN);
    const int src = shift_reg_is_a ? ga : gb;
    const int t = take(pins, npins, idx);
    if (t < 0) return false;
    if (ins.shift_type == SH_LSL && ins.shift_amt >= 1 && ins.shift_amt <= 3) {
      body_.lea(t, Mem::x(src, 1 << ins.shift_amt, 0));
    } else {
      body_.mov_rr(t, src);
      switch (ins.shift_type) {
        case SH_LSL: body_.shift_ri(S_SHL, t, ins.shift_amt); break;
        case SH_LSR: body_.shift_ri(S_SHR, t, ins.shift_amt); break;
        case SH_ASR: body_.shift_ri(S_SAR, t, ins.shift_amt); break;
        case SH_ROR: body_.shift_ri(S_ROR, t, ins.shift_amt); break;
        default: body_.shift_ri(S_RCR, t, 1); break;                   /* RRX */
      }
    }
    op2 = t;
    op2_temp = t;
  }
  if (pe_shift) body_.popfq();
  if (ps_op) body_.pushfq();

  /* The one ARM flag rule with no x86 instruction behind it: a flag-setting
   * logical operation with an unshifted operand leaves C alone, and every host
   * logical instruction clears CF.  `setcc` and `bt` are both flag-free, so
   * they can stash and restore the bit without touching the very flags they are
   * repairing. */
  if (carry_keep) body_.setcc(C_B, kScratch);

  /* --- the operation ---------------------------------------------------- */
  const bool al_a = !no_dst && reads_a && ins.op != IR_MOVI && dv == ga;
  const bool al_b = !no_dst && sh.reads_b && !imm_op && dv == gb;
  /* Two of the shapes below turn the operation around because the destination
   * already holds R(b) - and both of them need operand 2 to *be* R(b), not a
   * shifted copy of it.  A materialised shift (or RRX) breaks that: the second
   * operand is then a scratch register holding R(b) transformed. */
  const bool op2_is_gb = !imm_op && op2 == gb && gb >= 0;
  const bool commutative = ins.op == IR_AND || ins.op == IR_ORR || ins.op == IR_EOR ||
                           ins.op == IR_ADD || ins.op == IR_ADC;

  switch (ins.op) {
    case IR_MOVI:
      body_.mov_ri(dv, ins.imm);
      if (want) {
        body_.test_rr(dv, dv);
        if (carry_const) { if (ins.imm & 0x80000000u) body_.stc(); else body_.clc(); }
        else if (carry_keep) body_.bt_ri(kScratch, 0);
      }
      break;

    case IR_MOV:
    case IR_LSL:
    case IR_LSR:
    case IR_ASR:
    case IR_ROR:
    case IR_RRX: {
      if (dv != ga) body_.mov_rr(dv, ga);
      switch (ins.op) {
        case IR_MOV: break;                                            /* no shift */
        case IR_LSL:
          if (ins.shift_amt != 0) body_.shift_ri(S_SHL, dv, ins.shift_amt);
          break;
        case IR_LSR: body_.shift_ri(S_SHR, dv, ins.shift_amt); break;
        case IR_ASR: body_.shift_ri(S_SAR, dv, ins.shift_amt); break;
        case IR_ROR: body_.shift_ri(S_ROR, dv, ins.shift_amt); break;
        default: body_.shift_ri(S_RCR, dv, 1); break;                  /* RRX */
      }
      /* A left or right *shift* leaves ARM's carry in CF (bit 32-n or n-1) and
       * N, Z in SF, ZF, so those need no flag fixup at all.  The two cases that
       * do are the ones x86 does not compute for us:
       *   - `MOV` and `LSL #0` emit no shift at all, so N and Z have to come
       *     from a `test` - which clears CF - while C is the old carry, stashed
       *     in the scratch register before the operation (`carry_keep`);
       *   - `ROR`/`RRX` are *rotates*: x86 rotates set CF (the bit rotated out,
       *     exactly ARM's carry) but leave SF and ZF alone, so the result still
       *     needs a `test`, and the carry it just produced has to be rescued
       *     first.  `setcc` and `bt` are both flag-free, so a rotation can be
       *     rebuilt out of them without touching what the rotate produced. */
      if (want) {
        const bool real_shift = ins.op == IR_LSR || ins.op == IR_ASR ||
                                (ins.op == IR_LSL && ins.shift_amt != 0);
        if (!real_shift) {
          if (ins.op == IR_ROR || ins.op == IR_RRX) body_.setcc(C_B, kScratch);
          body_.test_rr(dv, dv);
          body_.bt_ri(kScratch, 0);
        }
      }
      break;
    }

    case IR_MVN: {
      if (mvn_imm) {
        /* ~imm, already rotated by the front end, and no source register to
         * load: the immediate form costs one `mov` and reads nothing. */
        body_.mov_ri(dv, ~ins.imm);
      } else {
        const int src = (op_shift && op2 != -1) ? op2 : ga;
        if (dv != src) body_.mov_rr(dv, src);
        body_.not_(dv);
      }
      if (want) {
        body_.test_rr(dv, dv);                 /* `not` sets nothing */
        if (carry_keep) body_.bt_ri(kScratch, 0);
        else if (carry_const) { if (ins.imm & 0x80000000u) body_.stc(); else body_.clc(); }
        else if (carry_bit)
          body_.bt_ri(carry_reg, shifter_carry_bit(ins.shift_type, ins.shift_amt));
      }
      break;
    }

    case IR_NEG:
      if (dv != ga) body_.mov_rr(dv, ga);
      body_.neg(dv);
      /* `neg` writes OF (and CF) whether or not ARM's S bit asked for them. */
      if (want || (def && lv.v_live_out)) body_.setcc(C_O, kVreg);
      break;

    case IR_AND:
    case IR_ORR:
    case IR_EOR: {
      /* Commutative, so a destination that happens to be the second operand is
       * computed the other way round - which also saves the `mov`. */
      if (no_dst && !op_shift && ins.op == IR_AND) {
        /* TST is the one discard-the-result form `test` matches exactly (CMP
         * has its own below); a TEQ still has to be an exclusive-or, so it
         * goes through the generic path into the dead destination. */
        if (imm_op) body_.test_ri(ga, ins.imm);
        else body_.test_rr(ga, op2);
      } else {
        if (al_b && !al_a && op2_is_gb) {
          if (imm_op) body_.alu_ri(ins.op == IR_AND ? A_AND : ins.op == IR_ORR ? A_OR : A_XOR,
                                   dv, ins.imm);
          else body_.alu_rr(ins.op == IR_AND ? A_AND : ins.op == IR_ORR ? A_OR : A_XOR,
                            dv, ga);
        } else {
          if (!al_a) body_.mov_rr(dv, ga);
          if (imm_op) body_.alu_ri(ins.op == IR_AND ? A_AND : ins.op == IR_ORR ? A_OR : A_XOR,
                                   dv, ins.imm);
          else body_.alu_rr(ins.op == IR_AND ? A_AND : ins.op == IR_ORR ? A_OR : A_XOR,
                            dv, op2);
        }
      }
      if (want) {
        if (carry_keep) body_.bt_ri(kScratch, 0);
        else if (carry_const) { if (ins.imm & 0x80000000u) body_.stc(); else body_.clc(); }
        else if (carry_bit)
          body_.bt_ri(carry_reg, shifter_carry_bit(ins.shift_type, ins.shift_amt));
      }
      break;
    }

    case IR_BIC: {
      /* dst = R(a) & ~op2.  The complement is built where it cannot destroy a
       * live guest value: in the destination when the destination *is* the
       * source register, otherwise in a scratch register of its own. */
      int src_a = ga;
      if (imm_op) {
        if (no_dst) { body_.mov_ri(dv, ga); body_.alu_ri(A_AND, dv, ~ins.imm); }
        else {
          if (!al_a) body_.mov_rr(dv, ga);
          body_.alu_ri(A_AND, dv, ~ins.imm);
        }
      } else if (dv == op2 && ga != dv) {
        /* The destination register holds ~op2 after this, which is the right
         * answer: it is the guest register being overwritten anyway.  It needs
         * R(a) to be a *different* register, though: `bic r0, r0, r0` puts the
         * destination, the first operand and the second operand in one place,
         * and complementing it would complement the value that is still to be
         * anded in. */
        body_.not_(dv);
        body_.alu_rr(A_AND, dv, ga);
      } else {
        const int t = take(pins, npins, idx);
        if (t < 0) return false;
        body_.mov_rr(t, op2);
        body_.not_(t);
        if (!al_a) body_.mov_rr(dv, src_a);
        body_.alu_rr(A_AND, dv, t);
        release_temp(t);
      }
      if (want) {
        if (carry_keep) body_.bt_ri(kScratch, 0);
        else if (carry_const) { if (ins.imm & 0x80000000u) body_.stc(); else body_.clc(); }
        else if (carry_bit)
          body_.bt_ri(carry_reg, shifter_carry_bit(ins.shift_type, ins.shift_amt));
      }
      break;
    }

    case IR_ADD: case IR_SUB: case IR_ADC: case IR_SBC: case IR_RSB: {
      const bool wflags = want;
      /* V has to be computed even when nothing reads the flags, if a later
       * instruction passes it on to the block's exit (see Liveness::v_live_out).
       * That rules out the flag-free encodings below: `lea` would leave the
       * carry-looking bits of OF stale, and `seto` reads OF. */
      const bool v_only = def && !wflags && lv.v_live_out;
      const int aop = ins.op == IR_ADD ? A_ADD : ins.op == IR_SUB ? A_SUB
                    : ins.op == IR_ADC ? A_ADC : ins.op == IR_SBC ? A_SBB : A_SUB;
      /* ARM's SBC subtracts `NOT C`, x86's SBB subtracts `CF`: the borrow-in
       * has to be complemented first.  (ADC needs no such care - both add C.)
       * A shifted operand cannot be in flight here: the front end refuses
       * ADC/SBC with a shift, because ARMv5's own rule for them is different. */
      if (ins.op == IR_SBC) body_.cmc();
      /* Flag-free encodings first: `lea` for addition and subtraction by an
       * immediate in one instruction, which is what keeps `add rd, rn, rm` out
       * of the pushfq/popfq path when the flags are live across it. */
      if (ins.op == IR_ADD && !wflags && !v_only) {
        if (imm_op) body_.lea(dv, Mem::bx(ga, (int32_t)ins.imm));
        else body_.lea(dv, Mem::bxi(ga, op2, 1, 0));
      } else if (ins.op == IR_SUB && !wflags && !v_only && imm_op &&
                 (int32_t)ins.imm != (int32_t)0x80000000u) {
        body_.lea(dv, Mem::bx(ga, -(int32_t)ins.imm));
      } else if (no_dst && ins.op == IR_SUB && !op_shift) {
        /* CMP has its own form: same flags as the subtraction, no destination */
        if (imm_op) body_.alu_ri(A_CMP, ga, ins.imm);
        else body_.alu_rr(A_CMP, ga, op2);
      } else if (ins.op == IR_RSB && imm_op && ins.imm == 0) {
        if (dv != ga) body_.mov_rr(dv, ga);
        body_.neg(dv);
      } else if (ins.op == IR_RSB && dv != ga) {
        if (imm_op) body_.mov_ri(dv, ins.imm);
        else if (al_b && op2_is_gb) { /* dv already holds R(b) */ }
        else body_.mov_rr(dv, op2);
        body_.alu_rr(A_SUB, dv, ga);
      } else if (ins.op == IR_RSB) {
        const int t = take(pins, npins, idx);
        if (t < 0) return false;
        if (imm_op) body_.mov_ri(t, ins.imm); else body_.mov_rr(t, op2);
        body_.alu_rr(A_SUB, t, ga);
        body_.mov_rr(dv, t);
        release_temp(t);
      } else if (al_b && commutative && op2_is_gb) {
        if (imm_op) body_.alu_ri(aop, dv, ins.imm); else body_.alu_rr(aop, dv, ga);
      } else if (al_b) {
        /* A non-commutative subtraction whose destination is R(b): R(b) has to
         * stay readable until it has been subtracted, so the difference is
         * computed in a scratch and moved. */
        const int t = take(pins, npins, idx);
        if (t < 0) return false;
        body_.mov_rr(t, ga);
        if (imm_op) body_.alu_ri(aop, t, ins.imm); else body_.alu_rr(aop, t, op2);
        body_.mov_rr(dv, t);
        release_temp(t);
      } else {
        if (!al_a) body_.mov_rr(dv, ga);
        if (imm_op) body_.alu_ri(aop, dv, ins.imm); else body_.alu_rr(aop, dv, op2);
      }
      if (wflags || v_only) {
        /* ARM's C out of a subtraction is the *inverse* of x86's borrow, and
         * `cmp` is a subtraction.  ADD/ADC (and NEG, which x86 defines as
         * "CF = operand != 0" - exactly ARM's NOT-borrow of 0 - x) already
         * agree, and `cmc` leaves SF, ZF and OF alone, so V is still `seto`. */
        if (wflags && (ins.op == IR_SUB || ins.op == IR_SBC || ins.op == IR_RSB))
          body_.cmc();
        body_.setcc(C_O, kVreg);
      }
      break;
    }

    default:
      return false;
  }

  if (pe_op) body_.popfq();
  if (no_dst && dv != kScratch) release_temp(dv);   /* flag-only form: done with it */
  if (op2_temp >= 0) release_temp(op2_temp);
  if (hold_reg >= 0) taken_[hold_reg] = false;      /* the superseded operand */
  return true;
}

/* ------------------------------------------------------- prologue / exit -- */

void Emitter::prologue() {
  body_.push64(kShadow);
  body_.mov_rr64(kShadow, RDI);
  /* The host flags are loaded only when the *guest's* flags at entry are read
   * by the block (an ADC/SBC/RRX, or a form that has to preserve them); V is
   * different: the block writes it back at exit even when it never reads it,
   * so it enters the block from the shadow whenever the exit writes flags. */
  if (exit_flags_) {
    body_.push64(kVreg);
    body_.mov_rm(kVreg, Mem::bx(kShadow, SH_VFLAG));
  }
  if (entry_flags_) {
    /* One load: the image in the shadow is literally what `popfq` wants. */
    body_.push_mem(Mem::bx(kShadow, SH_EFLAGS));
    body_.popfq();
  }
  for (int i = 0; i < kPoolCount; ++i)
    if (ever_[kPool[i]] && is_callee_saved_pool(kPool[i])) body_.push64(kPool[i]);
}

void Emitter::epilogue() {
  if (exit_flags_) {
    body_.pushfq();
    body_.pop64(RAX);
    body_.mov_mem_r(Mem::bx(kShadow, SH_EFLAGS), RAX);
    body_.mov_mem_r(Mem::bx(kShadow, SH_VFLAG), kVreg);
  }
  body_.mov_mem_i(Mem::bx(kShadow, SH_ABORT_PC), 0);
  body_.mov_mem_i(Mem::bx(kShadow, SH_R15), end_pc_);
  for (int i = kPoolCount - 1; i >= 0; --i)
    if (ever_[kPool[i]] && is_callee_saved_pool(kPool[i])) body_.pop64(kPool[i]);
  if (exit_flags_) body_.pop64(kVreg);
  body_.pop64(kShadow);
  body_.ret();
}

/* ------------------------------------------------------------------ run -- */

bool Emitter::run(std::vector<uint8_t>* out, JitBlockMeta* meta) {
  const uint32_t n = blk_.n;
  if (n == 0 || n > IR_MAX_INSNS) return false;
  reset();

  Liveness lv[IR_MAX_INSNS];
  compute_liveness(blk_, lv);

  /* The allocator's next-use table: use_[i][g] is the index of the next
   * instruction at or after i that mentions guest register g, plus one
   * (0 = never again). */
  for (int g = 0; g < kGuestRegs; ++g) use_[n][g] = 0;
  for (int i = (int)n - 1; i >= 0; --i) {
    for (int g = 0; g < kGuestRegs; ++g) use_[i][g] = use_[i + 1][g];
    const IrInsn& ins = in(i);
    const Shape sh = shape_of(ins);
    if (sh.reads_a && ins.op != IR_MOVI && !(ins.op == IR_MVN && ins.b_is_imm) &&
        ins.a < kGuestRegs)
      use_[i][ins.a] = (uint8_t)(i + 1);
    if (sh.reads_b && !ins.b_is_imm && ins.b < kGuestRegs) use_[i][ins.b] = (uint8_t)(i + 1);
    if (ins.dst != IR_NO_DST && ins.dst < kGuestRegs) use_[i][ins.dst] = (uint8_t)(i + 1);
  }

  entry_flags_ = lv[0].live_in;
  exit_flags_ = lv[n - 1].live_out;
  end_pc_ = blk_.next_pc;

  for (uint32_t i = 0; i < n; ++i) {
    if (!jit_x64_accepts(in((int)i))) return false;
    if (!op((int)i, lv[i], shape_of(in((int)i)))) return false;
  }
  for (int g = 0; g < kGuestRegs; ++g)
    if (host_of_[g] >= 0 && dirty_[g]) spill(g);

  /* The prologue is emitted after the body: only then does it know which
   * callee-saved registers the allocator reached for, and the body can be
   * discarded wholesale if an instruction turns out not to be emittable. */
  Buf code;
  std::swap(code.w, body_.w);
  body_.w.clear();
  prologue();
  body_.w.insert(body_.w.end(), code.w.begin(), code.w.end());
  epilogue();

  out->assign(body_.w.begin(), body_.w.end());
  meta->entry_flags = entry_flags_ ? 1 : 0;
  meta->exit_flags = exit_flags_ ? 1 : 0;
  return true;
}

}  // namespace

bool jit_emit_x64(const IrBlock& blk, bool with_entry_exit, std::vector<uint8_t>* out,
                  JitBlockMeta* meta) {
  out->clear();
  Emitter e(blk);
  if (!with_entry_exit) {
    /* Without the entry/exit code a block is not callable, and nothing in this
     * project wants one: refusing loudly beats emitting a fragment. */
    return false;
  }
  return e.run(out, meta);
}

}  // namespace brain
