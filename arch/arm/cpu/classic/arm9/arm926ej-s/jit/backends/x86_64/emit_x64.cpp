/* x86-64 code generation for the JIT's IR.  See jit/backends/x86_64/emit_x64.h for the register
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
#include "arch/arm/cpu/classic/arm9/arm926ej-s/jit/backends/x86_64/emit_x64.h"

#include <cstddef>
#include <cstring>

#include "arch/arm/cpu/classic/arm9/arm926ej-s/core/state.h"   /* SH_* : the shift-type numbering the IR uses */
#include "arch/arm/cpu/classic/arm9/arm926ej-s/jit/abi.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/jit/backends/x86_64/x64_asm.h"

namespace brain::arm926 {

/* The shift-type numbering (SH_LSL..SH_RRX) the IR uses is the ISA layer's
 * (arch/arm/isa/armv5/state.h). */
using namespace brain::arm::isa::armv5;

/* Per-op acceptance, consulted by the front end before it appends an
 * instruction to a block (see jit/frontend.cpp). */
bool jit_x64_accepts(const IrInsn& in) {
  switch (in.op) {
    case IR_MOV: case IR_MOVI: case IR_ADD: case IR_SUB: case IR_RSB:
    case IR_ADC: case IR_SBC: case IR_RSC: case IR_AND: case IR_ORR: case IR_EOR:
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
    case IR_LD: case IR_ST:
      /* Memory: the register window, the addressing form and the shift of a
       * register offset.  The *machine* conditions that make an inlined access
       * legal (RAM, no alignment check, no big-endian data) are the front end's
       * decision, because they are about the guest, not about x86-64. */
      if (in.mem_size != 1 && in.mem_size != 2 && in.mem_size != 4) return false;
      if (in.mem_reg > MEM_REG || in.mem_post > 1 || in.mem_wb > MEM_WB_YES) return false;
      if (in.dst >= 13) return false;                       /* value register */
      if (in.mem_shadow ? (in.a < 13 || in.a > 14) : in.a >= 13) return false;
      if (in.mem_wb != MEM_WB_NONE && in.mem_reg == MEM_ABS) return false;
      if (in.mem_reg == MEM_REG) {
        if (in.b >= 13) return false;                       /* never R13/R14 */
        if (in.shift_type == SH_RRX) return false;          /* no RRX index    */
        if (in.shift_type != SH_LSL && in.shift_amt == 0) return false;  /* #32 */
        if (in.shift_amt > 31) return false;
      } else if (in.shift_amt != 0) {
        return false;
      }
      break;
    case IR_BR:
      /* Control flow: only the four ARM condition codes the host cannot test
       * directly need care, and those are handled in the exit path below. */
      if (in.cond > 0xEu) return false;
      if (in.br_link && in.br_reg && in.b >= 13) return false;
      if (in.br_reg && in.b >= 15) return false;
      if (in.br_mask != BR_MASK_STATE && in.br_mask != BR_MASK_HALF &&
          in.br_mask != BR_MASK_WORD)
        return false;
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
    case IR_RSC:
      s.reads_a = true; s.reads_b = true; s.arith = true;
      break;
    case IR_LD: case IR_ST:
      /* The address is an operand; a store also reads the register it stores.
       * Both are emitted by mem_op(), which never reaches the data path below -
       * `mem` exists so the allocator's next-use table sees the right operands. */
      s.mem = true;
      s.reads_a = !in.mem_shadow;
      s.reads_b = in.mem_reg == MEM_REG;
      break;
    case IR_BR:
      /* The branch is the exit path; nothing of it is emitted in the body. */
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
  return in.op == IR_ADC || in.op == IR_SBC || in.op == IR_RSC ||
         in.op == IR_RRX ||
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

/* A conditional branch *reads* the flags without producing a value: its ARM
 * condition is evaluated on N, Z, C and V, so the flags are live into it and V
 * has to be real there.  This is what keeps the host flags correct up to the
 * exit path (see the note in Emitter::exit_path). */
bool reads_flags(const IrInsn& in) {
  return in.op == IR_BR && in.cond != 0xEu;
}

/* ARM conditions whose test involves V: VS, VC, GE, LT, GT, LE.  V is the one
 * flag that does not live in the host flag register (see the header), so a
 * block that tests one of these has to have the V word loaded at entry. */
bool cond_uses_v(uint32_t cond) {
  return cond == 0x6 || cond == 0x7 || cond == 0xA || cond == 0xB ||
         cond == 0xC || cond == 0xD;
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
    /* A memory access can *end the block*: every guard around it is a bail
     * site, and the guest flags the interpreter would observe after the retired
     * prefix are the ones live at the access.  So an access observes the flags
     * exactly like the block exit does, and the propagation below then keeps
     * them in a host register all the way back to their writer - without this,
     * an access between a flag writer and the exit could clobber the host flags
     * inside a guard and the bail would hand the dispatcher the guard's
     * comparison instead of the guest's flags (which the guard's own pushfq
     * preserves, but only when this says the value is wanted). */
    if (in.op == IR_LD || in.op == IR_ST) { live = true; v = true; }
    /* Live going in: something after needs the flags (if this instruction does
     * not write them, or writes them as a copy of the old ones), or this
     * instruction needs the carry for its own result. */
    lv[i].live_out = live;
    live = (live && (!def || copies_carry_through(in))) || reads_carry_for_value(in) ||
           reads_flags(in);
    lv[i].live_in = live;
    /* V again, from the same point of view: an arithmetic op defines V (one
     * `seto`, which the emitter will emit whenever either variable says the
     * value is wanted), and every other op - including a flag-setting logical,
     * which ARM leaves V alone for - passes the register through. */
    lv[i].v_live_out = v;
    if (reads_flags(in)) v = true;      /* the branch observes the incoming V */
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
  bool mem_op(int idx, const Liveness& lv, const Shape& sh);
  void add_bail(int cc, int idx, bool flags_pushed);
  void branch_target(int reg, uint8_t mask);
  void prologue();
  bool exit_path();
  void exit_tail();
  void emit_bails();
  void store_flags();
  bool emit_cond(uint32_t cond, std::vector<size_t>* taken,
                 std::vector<size_t>* not_taken);
  int nv_xor(int at);

  const IrInsn& in(int i) const { return blk_.in[i]; }

  /* A guest access the block must not perform - the address left the identity
   * RAM window, the access would cross into the next page, or a store's page
   * holds translated code - leaves through one of these.  The site records what
   * the block had modified at that point, so the cold path can hand the
   * dispatcher a machine state that is exactly the one the interpreter would
   * have after `retired` instructions. */
  struct Bail {
    size_t patch_at = 0;       /* the guard jump that lands here            */
    uint32_t resume_pc = 0;    /* where the interpreter picks the guest up  */
    uint32_t retired = 0;
    uint32_t cycles = 0;
    bool flags_pushed = false; /* a pushfq of the preserve window is open   */
    int host_of[kGuestRegs];
    bool dirty[kGuestRegs];
  };

  const IrBlock& blk_;
  Buf body_;
  int host_of_[kGuestRegs];
  bool dirty_[kGuestRegs];
  uint8_t use_[IR_MAX_INSNS + 2][kGuestRegs];
  bool taken_[16];
  bool ever_[16];
  bool entry_flags_ = false;
  bool exit_flags_ = false;
  bool v_read_ = false;      /* a branch tests V: it has to be loaded      */
  bool v_loaded_ = false;    /* ...and so saved/restored around the block  */
  uint32_t end_pc_ = 0;
    size_t tail_at_ = 0;
    size_t pop_at_ = 0;        /* the tail *after* the ABORT_PC clear, where
                                * a bail site with its own resume address
                                * joins (see exit_tail/emit_bails)          */
    std::vector<Bail> bails_;
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

/* ------------------------------------------------ the way out of a block -- */

/* Remember a bail: the guard jump is emitted here, the cold path later (see
 * emit_bails).  `cc` is the host condition that fails. */
/* The address a block hands back for a *branch target held in a register*.  The
 * low bit of what it writes is the state the machine continues in - which is
 * also what bit 0 of the guest's own `bx`/`blx` target means, so the two rules
 * are one:
 *
 *   BR_MASK_STATE: the value goes out *untouched*.  The dispatcher reads bit 0
 *                  as the state and then drops the address bits that state
 *                  requires (bit 0 for a Thumb target, bits[1:0] for an ARM one)
 *                  - which is exactly ARM's masking rule, in one place instead
 *                  of two;
 *   BR_MASK_HALF : a Thumb target the encoding already decided: force bit 0 to
 *                  one, so the dispatcher strips it and switches to Thumb;
 *   BR_MASK_WORD : a BLX leaving Thumb for ARM: drop bits[1:0], so bit 0 reads
 *                  zero and the dispatcher switches (and stays) in ARM state.
 *
 * Masking here rather than in the dispatcher would break the first case: the
 * bit that says which mask to use is the bit a mask removes. */
void Emitter::branch_target(int reg, uint8_t mask) {
  body_.mov_rm(RAX, Mem::bx(kShadow, SH_R0 + 4 * reg));
  if (mask == BR_MASK_HALF) body_.alu_ri32(A_OR, RAX, 1u);
  else if (mask == BR_MASK_WORD) body_.alu_ri32(A_AND, RAX, 0xFFFFFFFCu);
  body_.mov_mem_r(Mem::bx(kShadow, SH_R15), RAX);
}

void Emitter::add_bail(int cc, int idx, bool flags_pushed) {
  Bail b;
  b.patch_at = body_.jcc32(cc);
  b.resume_pc = blk_.start_pc + (blk_.thumb ? 2u : 4u) * (uint32_t)idx;
  if (blk_.thumb) b.resume_pc |= 1u;    /* the interpreter resumes in Thumb */
  b.retired = (uint32_t)idx;
  b.cycles = idx > 0 ? in(idx - 1).cyc : 0u;
  b.flags_pushed = flags_pushed;
  for (int g = 0; g < kGuestRegs; ++g) {
    b.host_of[g] = host_of_[g];
    b.dirty[g] = dirty_[g];
  }
  bails_.push_back(b);
}

/* N xor V in a register, with ZF = (N == V) on return: the test GE/LT/GT/LE
 * need, and the reason they cost a few instructions rather than one.  `setcc`
 * and `movzx` are flag-free, but the `xor`/`test` are not - which is why the
 * exit stores the guest's flag image *before* calling this. */
int Emitter::nv_xor(int at) {
  const int t = take(nullptr, 0, at);
  if (t < 0) return -1;
  const int u = take(nullptr, 0, at);
  if (u < 0) { release_temp(t); return -1; }
  body_.setcc(C_S, t);                  /* N */
  body_.movzx_u8(t, t);
  body_.movzx_u8(u, kVreg);             /* V, as a 0/1 word */
  body_.alu_rr(A_XOR, t, u);
  body_.test_rr(t, t);                  /* ZF = (N == V) */
  release_temp(u);
  return t;
}

/* The host flags hold N, Z and C and r15d holds V, so all fourteen ARM
 * conditions are testable: the six that also involve V (VS, VC and the four
 * signed comparisons) are built from `nv_xor`, the rest map onto one `jcc`
 * directly - and the two that would need *two* flag tests taken together (HI,
 * LS) are emitted as jumps to the *not-taken* side, which is one jump each
 * because falling through to the taken side is the default. */
bool Emitter::emit_cond(uint32_t cond, std::vector<size_t>* taken,
                        std::vector<size_t>* not_taken) {
  const int at = (int)blk_.n - 1;
  switch (cond) {
    case 0x0: taken->push_back(body_.jcc32(C_E)); return true;         /* EQ */
    case 0x1: taken->push_back(body_.jcc32(C_NE)); return true;        /* NE */
    case 0x2: taken->push_back(body_.jcc32(C_B)); return true;         /* CS */
    case 0x3: taken->push_back(body_.jcc32(C_AE)); return true;        /* CC */
    case 0x4: taken->push_back(body_.jcc32(C_S)); return true;         /* MI */
    case 0x5: taken->push_back(body_.jcc32(C_NS)); return true;        /* PL */
    case 0x6: case 0x7: {                                              /* VS/VC */
      const int t = take(nullptr, 0, at);
      if (t < 0) return false;
      body_.movzx_u8(t, kVreg);
      body_.test_rr(t, t);
      taken->push_back(body_.jcc32(cond == 0x6 ? C_NE : C_E));
      release_temp(t);
      return true;
    }
    /* HI and LS are the only two conditions that are not a single x86 flag
     * test: one is "C and not Z" and the other its complement.  CF here *is*
     * ARM's C - the entry image (`eflags_image`) maps bit 29 onto CF directly,
     * and the flag-setting arithmetic keeps that true with its `cmc` - so
     *   HI = C and not Z  ->  out if C is clear, else in if Z is clear;
     *   LS = not C or Z   ->  in if C is clear, else in if Z is set.
     * Only a `taken` jump reaches the taken block: falling out of the test
     * sequence lands on the jump over it, so a condition built out of
     * `not_taken` jumps alone could never take the branch (which is how HI
     * used to be spelled - it read as the negation of LS). */
    case 0x8:                                                          /* HI */
      not_taken->push_back(body_.jcc32(C_AE));   /* C clear: out             */
      taken->push_back(body_.jcc32(C_NE));       /* Z clear: in              */
      return true;                               /* C set, Z set: out        */
    case 0x9:                                                          /* LS */
      taken->push_back(body_.jcc32(C_AE));       /* C clear: in              */
      taken->push_back(body_.jcc32(C_E));        /* or Z set: in             */
      return true;                               /* else out                 */
    case 0xA: case 0xB: {                                              /* GE/LT */
      const int t = nv_xor(at);
      if (t < 0) return false;
      taken->push_back(body_.jcc32(cond == 0xA ? C_E : C_NE));
      release_temp(t);
      return true;
    }
    case 0xC: {                                                        /* GT */
      not_taken->push_back(body_.jcc32(C_E));
      const int t = nv_xor(at);
      if (t < 0) return false;
      taken->push_back(body_.jcc32(C_E));
      release_temp(t);
      return true;
    }
    default: {                                                         /* LE */
      taken->push_back(body_.jcc32(C_E));
      const int t = nv_xor(at);
      if (t < 0) return false;
      taken->push_back(body_.jcc32(C_NE));
      release_temp(t);
      return true;
    }
  }
}

/* The guest's flag image for the dispatcher: exactly the handoff the original
 * straight-line backend used, now run on every path out of the block. */
void Emitter::store_flags() {
  body_.pushfq();
  body_.pop64(RAX);
  body_.mov_mem_r(Mem::bx(kShadow, SH_EFLAGS), RAX);
  body_.mov_mem_r(Mem::bx(kShadow, SH_VFLAG), kVreg);
}

/* --------------------------------------------------------------- memory -- */

/* A guest load or store, done inline against the identity RAM window the front
 * end baked into the IR.  Three guards stand between the address and the
 * access, and each one *ends the block* instead of guessing:
 *
 *   - the access must not cross a page boundary (so one page test covers it);
 *   - the address must be inside the RAM window (MMU off, no device, no ROM -
 *     the same test the interpreter's own fast path makes);
 *   - a store must not land in a page that holds translated code: that store
 *     goes back to the interpreter, whose write path is the one that tells the
 *     cache to drop stale blocks (`docs/MODULES.md`: every writer reports).
 *
 * What is left is a plain unaligned host load/store, matching the interpreter's
 * identity path bit for bit - including the unaligned word access, where both
 * read the four bytes at the address.
 */
bool Emitter::mem_op(int idx, const Liveness& lv, const Shape& sh) {
  (void)sh;
  const IrInsn& ins = in(idx);
  const bool is_load = ins.op == IR_LD;
  const bool reg_off = ins.mem_reg == MEM_REG;
  const bool pres = lv.live_in;       /* an access never writes the flags...
                                       * but its address arithmetic does     */
  const int npins_max = 3;
  int pins[npins_max];
  int npins = 0;
  if (!ins.mem_shadow) pins[npins++] = (int)ins.a;
  if (reg_off) pins[npins++] = (int)ins.b;
  if (!is_load) pins[npins++] = (int)ins.dst;

  const int gb = ins.mem_shadow ? -1 : need((int)ins.a, pins, npins, idx, false);
  if (!ins.mem_shadow && gb < 0) return false;
  const int gv = is_load ? -1 : need((int)ins.dst, pins, npins, idx, false);
  if (!is_load && gv < 0) return false;
  const int gi = reg_off ? need((int)ins.b, pins, npins, idx, false) : -1;
  if (reg_off && gi < 0) return false;

  const int ea = take(pins, npins, idx);          /* the effective address  */
  const int t2 = ea >= 0 ? take(pins, npins, idx) : -1;
  const int rp = t2 >= 0 ? take(pins, npins, idx) : -1;   /* the RAM base    */
  if (rp < 0) {
    if (ea >= 0) release_temp(ea);
    if (t2 >= 0) release_temp(t2);
    return false;
  }
  /* `gp` is the register the block's *base* ends up in for a writeback: the
   * allocator's slot for R(a), or (for R13/R14) a scratch that is then stored
   * into the shadow. */
  const int shadow_base = SH_R0 + 4 * (int)ins.a;
  const bool writeback = ins.mem_wb != MEM_WB_NONE;
  const bool post = ins.mem_post != 0;

  /* Materialise the shifted index register into `d`: LSL #1..#3 is one
   * flag-free `lea`, anything else is a host shift (the address arithmetic is
   * inside the preserve window, so clobbering the flags is allowed there). */
  auto materialise_index = [&](int d) {
    if (ins.shift_amt == 0) {
      body_.mov_rr(d, gi);
    } else if (ins.shift_type == SH_LSL && ins.shift_amt <= 3) {
      body_.lea(d, Mem::x(gi, 1 << ins.shift_amt, 0));
    } else {
      body_.mov_rr(d, gi);
      switch (ins.shift_type) {
        case SH_LSL: body_.shift_ri(S_SHL, d, ins.shift_amt); break;
        case SH_LSR: body_.shift_ri(S_SHR, d, ins.shift_amt); break;
        case SH_ASR: body_.shift_ri(S_SAR, d, ins.shift_amt); break;
        default: body_.shift_ri(S_ROR, d, ins.shift_amt); break;
      }
    }
  };

  /* One pushfq covers every clobbering instruction below, including the
   * writeback - the load or store itself is flag-free.  A bail path pops it
   * before it reads anything (see emit_bails). */
  /* `d += / -= offset`, the one expression both the pre-indexed effective
   * address and the post-indexed writeback value are made of. */
  auto apply_offset = [&](int d, int tmp) {
    if (reg_off) {
      materialise_index(tmp);
      body_.alu_rr(ins.mem_sub ? A_SUB : A_ADD, d, tmp);
    } else if (ins.imm != 0) {
      body_.alu_ri(ins.mem_sub ? A_SUB : A_ADD, d, ins.imm);
    }
  };

  if (pres) body_.pushfq();

  /* The address the access uses.  A *pre*-indexed form applies the offset to
   * the base - that is the effective address, and what a writeback stores.  A
   * post-indexed one accesses the base itself and only then applies the offset,
   * so the offset is left to the writeback step below; computing it here would
   * access the wrong address and write the base back twice over. */
  if (ins.mem_reg == MEM_ABS) {
    body_.mov_ri(ea, ins.imm);
  } else {
    if (ins.mem_shadow) body_.mov_rm(ea, Mem::bx(kShadow, shadow_base));
    else body_.mov_rr(ea, gb);
    if (!post) apply_offset(ea, t2);
  }

  /* Guard 0: accesses are allowed at all right now.  This is the verdict the
   * front end asked once per block (`jit_fast_mem_ok`), written by the
   * dispatcher at every entry and re-tested here, so a block that is entered
   * with a machine that no longer matches the translation (a test driving the
   * cache by hand, a bus window that moved between the entry check and the
   * access) leaves through the interpreter instead of touching memory. */
  if (blk_.uses_mem) {
    body_.mov_rm(t2, Mem::bx(kShadow, SH_MEM_FAST));
    body_.test_rr(t2, t2);
    add_bail(C_E, idx, pres);
  }

  /* Guard 1: the access must stay inside one page (the RAM window test below
   * only looks at the first address, and the code-page test below only at one
   * page, so a straddling access - rare, and legal - goes to the interpreter).
   * A byte access cannot straddle. */
  if (ins.mem_size > 1) {
    body_.mov_rr(t2, ea);
    body_.alu_ri(A_AND, t2, 0xFFFu);
    body_.alu_ri(A_CMP, t2, 0x1000u - ins.mem_size);
    add_bail(C_A, idx, pres);
  }
  /* Guard 2: the identity RAM window, tested exactly as `Cpu::mem_read` does. */
  body_.mov_rr(t2, ea);
  body_.alu_ri(A_SUB, t2, blk_.mem_gbase);
  body_.alu_ri(A_CMP, t2, blk_.mem_mask);
  add_bail(C_A, idx, pres);
  /* Guard 3: a store into a page that might hold translated code. */
  if (!is_load) {
    body_.mov_rr(t2, ea);
    body_.shift_ri(S_SHR, t2, 12);
    body_.mov_rm64(rp, Mem::bx(kShadow, SH_CODEMAP));
    body_.cmp_m8_ri(Mem::bxi(rp, t2, 1, 0), 0);
    add_bail(C_NE, idx, pres);
  }

  /* The access: the address becomes an offset into the RAM window (`addr &
   * mask`, the same expression the interpreter's fast path uses), and the RAM
   * base pointer arrives in a register of its own. */
  body_.mov_rr(t2, ea);
  body_.alu_ri(A_AND, t2, blk_.mem_mask);
  body_.mov_rm64(rp, Mem::bx(kShadow, SH_RAM));
  if (is_load) {
    const int dv = host_of_[ins.dst] >= 0 ? host_of_[ins.dst]
                                         : take(pins, npins, idx);
    if (dv < 0) return false;
    host_of_[ins.dst] = dv;
    dirty_[ins.dst] = true;
    const Mem m = Mem::bxi(rp, t2, 1, 0);
    if (ins.mem_signed && ins.mem_size == 1) body_.movsx_u8_m(dv, m);
    else if (ins.mem_signed) body_.movsx_u16_m(dv, m);
    else if (ins.mem_size == 4) body_.mov_rm(dv, m);
    else if (ins.mem_size == 2) body_.movzx_u16_m(dv, m);
    else body_.movzx_u8_m(dv, m);
  } else {
    const Mem m = Mem::bxi(rp, t2, 1, 0);
    if (ins.mem_size == 4) body_.mov_mem_r(m, gv);
    else if (ins.mem_size == 2) body_.mov_mem16_r(m, gv);
    else body_.mov_mem8_r(m, gv);
  }

  /* Writeback, last - the interpreter stores before it updates the base, which
   * is what makes `str r0, [r0], #4` write the *old* r0.  A pre-indexed access
   * writes the effective address, a post-indexed one the offset applied to the
   * base; either way the value is in `ea` or one add away from it, and the
   * pool register is the allocator's slot for R(a). */
  if (writeback) {
    const int wv = ins.mem_shadow ? t2 : gb;
    if (post) {
      /* base ± offset, computed from the base the access just used.  The
       * scratch is `rp` when the writeback slot *is* the other scratch (`t2`),
       * which is what a shadow base - R13/R14 - gets. */
      body_.mov_rr(wv, ea);
      apply_offset(wv, wv == t2 ? rp : t2);
    } else if (wv != ea) {
      body_.mov_rr(wv, ea);
    }
    if (ins.mem_shadow) body_.mov_mem_r(Mem::bx(kShadow, shadow_base), wv);
    else dirty_[ins.a] = true;
  }

  if (pres) body_.popfq();
  release_temp(ea);
  release_temp(t2);
  release_temp(rp);
  return true;
}

/* --------------------------------------------------------------- one op -- */

bool Emitter::op(int idx, const Liveness& lv, const Shape& sh) {
  if (in(idx).op == IR_LD || in(idx).op == IR_ST) return mem_op(idx, lv, sh);
  if (in(idx).op == IR_BR) {
    /* A branch is entirely the exit path's business (the address it selects,
     * the link it writes, the condition it tests): nothing is emitted here. */
    return true;
  }
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

  /* --- the borrow-in, for RSC ------------------------------------------- */
  /* `sbb` subtracts the *host* CF, and at this point CF holds the guest's carry
   * as it came in - but ARM's RSC subtracts NOT(C), so the complement has to be
   * in place when the subtraction runs (the same `cmc` the SBC path does, and
   * the mirrored `sub` form does not need).  The only thing between here and
   * there that can destroy CF is the materialisation of a shifted operand, so
   * for those shapes the guest's carry is stashed first, flag-free, in the
   * emitter's own scratch register; `bt` puts it back below. */
  bool rsc_restore_cf = false;
  if (ins.op == IR_RSC && op_shift && !shift_flag_free) {
    body_.setcc(C_B, kScratch);
    rsc_restore_cf = true;
  }

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
  if (ins.op == IR_RSC) {
    if (rsc_restore_cf) body_.bt_ri(kScratch, 0);   /* CF = the guest's C  */
    body_.cmc();                                    /* CF = NOT(C)         */
  }

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

    case IR_ADD: case IR_SUB: case IR_ADC: case IR_SBC: case IR_RSB:
    case IR_RSC: {
      const bool wflags = want;
      /* V has to be computed even when nothing reads the flags, if a later
       * instruction passes it on to the block's exit (see Liveness::v_live_out).
       * That rules out the flag-free encodings below: `lea` would leave the
       * carry-looking bits of OF stale, and `seto` reads OF. */
      const bool v_only = def && !wflags && lv.v_live_out;
      const int aop = ins.op == IR_ADD ? A_ADD : ins.op == IR_SUB ? A_SUB
                    : ins.op == IR_ADC ? A_ADC
                    : (ins.op == IR_SBC || ins.op == IR_RSC) ? A_SBB : A_SUB;
      /* ARM's SBC and RSC subtract `NOT C`, x86's SBB subtracts `CF`: the
       * borrow-in has to be complemented first.  (ADC needs no such care - both
       * add C.)  RSC is the same subtraction with the minuend in the *second*
       * operand, so it takes the same `sbb` and only swaps which register is
       * the destination - see the reversed-operand branches below.
       * A shifted operand cannot be in flight for SBC: the front end refuses it,
       * because ARMv5's own rule for the pair is different. */
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
      } else if ((ins.op == IR_RSB || ins.op == IR_RSC) && dv != ga) {
        if (imm_op) body_.mov_ri(dv, ins.imm);
        else if (al_b && op2_is_gb) { /* dv already holds R(b) */ }
        else body_.mov_rr(dv, op2);
        body_.alu_rr(aop, dv, ga);
      } else if (ins.op == IR_RSB || ins.op == IR_RSC) {
        const int t = take(pins, npins, idx);
        if (t < 0) return false;
        if (imm_op) body_.mov_ri(t, ins.imm); else body_.mov_rr(t, op2);
        body_.alu_rr(aop, t, ga);
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
        if (wflags && (ins.op == IR_SUB || ins.op == IR_SBC ||
                       ins.op == IR_RSB || ins.op == IR_RSC))
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
   * by the block (an ADC/SBC/RRX, a form that has to preserve them, or a
   * conditional branch).  V is loaded whenever the block stores it back *or*
   * tests it - a VS/VC/GE/LT/GT/LE branch - and the pair is saved and restored
   * whenever it was loaded at all, so a block that never touches r15d leaves
   * the caller's copy alone. */
  if (v_loaded_) {
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

/* Everything from the end of the body to the `ret`.  Three things can be at the
 * end of a block, and the order in which they are emitted is what the whole
 * handoff rests on:
 *
 *   - the address execution resumes at.  `blk.next_pc` is the fall-through
 *     (after the last instruction, or the instruction after a conditional
 *     branch), and an unconditional branch has already put its target there;
 *     a conditional branch writes the fall-through first and rewrites it on the
 *     taken path;
 *   - how much of the block retired and what it cost.  A branch always retires
 *     the whole block, so those are constants here; a bail site writes its own
 *     (see add_bail);
 *   - the flags, as the EFLAGS image and the V word the dispatcher merges.  On
 *     a conditional branch this has to happen *before* the condition is
 *     evaluated, because GE/LT/GT/LE use the host flags as scratch to compare N
 *     with V.
 *
 * The tail is shared: the main paths fall into it, and the bail sites jump to
 * it with the resume address already written. */
bool Emitter::exit_path() {
  const IrInsn* br = (blk_.n && blk_.in[blk_.n - 1].op == IR_BR)
                         ? &blk_.in[blk_.n - 1] : nullptr;
  const bool cond_br = br && br->cond != 0xEu;

  if (!cond_br) {
    /* One path out: a constant address, an unconditional link write (ARM only
     * writes LR when the branch executes, and this one always does), and the
     * flags in the shadow before the return. */
    if (br && br->br_link)
      body_.mov_mem_i(Mem::bx(kShadow, SH_R0 + 4 * 14), br->link);
    if (br) body_.mov_mem_i(Mem::bx(kShadow, SH_BRANCH), 1);  /* taken: it ran */
    if (br && br->br_reg) {
      /* `bx`/`blx Rm` with no condition: the address is not a constant of the
       * translation, so it comes out of the shadow - where the spill pass just
       * put every guest register the block touched - exactly as on the taken
       * side of a conditional branch. */
      branch_target((int)br->b, br->br_mask);
    } else {
      body_.mov_mem_i(Mem::bx(kShadow, SH_R15), end_pc_);
    }
    body_.mov_mem_i(Mem::bx(kShadow, SH_RETIRED), blk_.n);
    body_.mov_mem_i(Mem::bx(kShadow, SH_CYCLES), blk_.cycles);
    if (exit_flags_) store_flags();
    exit_tail();
  } else {
    body_.mov_mem_i(Mem::bx(kShadow, SH_R15), end_pc_);      /* fall-through */
    body_.mov_mem_i(Mem::bx(kShadow, SH_BRANCH), 0);         /* not taken...  */
    body_.mov_mem_i(Mem::bx(kShadow, SH_RETIRED), blk_.n);
    body_.mov_mem_i(Mem::bx(kShadow, SH_CYCLES), br->cyc_nt); /* ...for free  */
    if (exit_flags_) store_flags();
    std::vector<size_t> taken, not_taken;
    if (!emit_cond(br->cond, &taken, &not_taken)) return false;
    /* The not-taken side falls through to the jump over the taken block; the
     * taken side is emitted after it and jumps to the tail itself.  Note which
     * byte the `jmp` starts at as well as its operand: a not-taken jump has to
     * *execute* that jmp, so it must land on the opcode, while the jmp's own
     * operand is what patch_rel32 wants. */
    const size_t jmp_start = body_.pos();
    const size_t jmp_at = body_.jmp32();
    const size_t taken_at = body_.pos();
    for (size_t at : taken) body_.patch_rel32(at, taken_at);
    for (size_t at : not_taken) body_.patch_rel32(at, jmp_start);
    body_.mov_mem_i(Mem::bx(kShadow, SH_BRANCH), 1);         /* ...until here */
    body_.mov_mem_i(Mem::bx(kShadow, SH_CYCLES), blk_.cycles);   /* the taken
                                     * branch restarts the prefetch stage, and
                                     * that extra cycle is the only difference
                                     * between the two exits */
    if (br->br_link) body_.mov_mem_i(Mem::bx(kShadow, SH_R0 + 4 * 14), br->link);
    if (br->br_reg) {
      /* A register branch (`bx`, `blx Rm`): the target is read out of the
       * shadow, where the exit's spill pass has just put every guest register
       * the block touched, and the *low bit is kept* - the dispatcher decodes
       * it as the ARM/Thumb switch, exactly as the interpreter does. */
      branch_target((int)br->b, br->br_mask);
    } else {
      body_.mov_mem_i(Mem::bx(kShadow, SH_R15), br->imm);
    }
    body_.patch_rel32(jmp_at, body_.pos());
    exit_tail();
  }
  emit_bails();
  return true;
}

void Emitter::exit_tail() {
  tail_at_ = body_.pos();
  body_.mov_mem_i(Mem::bx(kShadow, SH_ABORT_PC), 0);
  /* A bail jumps here instead: ABORT_PC has already been written with the
   * address it hands back, so it must not fall through the clear above. */
  pop_at_ = body_.pos();
  for (int i = kPoolCount - 1; i >= 0; --i)
    if (ever_[kPool[i]] && is_callee_saved_pool(kPool[i])) body_.pop64(kPool[i]);
  if (v_loaded_) body_.pop64(kVreg);
  body_.pop64(kShadow);
  body_.ret();
}

/* The cold paths of the accesses the block could not perform.  A bail has to
 * leave the machine in exactly the state the interpreter would have after
 * `retired` instructions, so it writes the registers the block had modified
 * (from the host registers, as they were when the guard failed), the flags, and
 * then the resume address - and joins the shared tail. */
void Emitter::emit_bails() {
  for (const Bail& b : bails_) {
    body_.patch_rel32(b.patch_at, body_.pos());
    if (b.flags_pushed) body_.popfq();
    for (int g = 0; g < kGuestRegs; ++g)
      if (b.dirty[g] && b.host_of[g] >= 0)
        body_.mov_mem_r(Mem::bx(kShadow, SH_R0 + 4 * g), b.host_of[g]);
    /* The flags are written back only when the pushfq above captured the
     * guest's own: without it the host flags at this point are the guard's
     * comparison, and the shadow still holds the entry image, which *is* the
     * guest's (liveness only lets an access sit between a flag writer and an
     * observer when it pushed, so flags_pushed covers exactly the case where
     * the last writer's value has to come from the host). */
    if (b.flags_pushed) store_flags();
    body_.mov_mem_i(Mem::bx(kShadow, SH_R15), b.resume_pc);
    body_.mov_mem_i(Mem::bx(kShadow, SH_RETIRED), b.retired);
    body_.mov_mem_i(Mem::bx(kShadow, SH_CYCLES), b.cycles);
    /* A bail is not the end of the block: the dispatcher has to see it and run
     * the refused instruction in the interpreter, which is what ABORT_PC (the
     * address to resume at, still set) tells it. */
    body_.mov_mem_i(Mem::bx(kShadow, SH_ABORT_PC), b.resume_pc);
    const size_t at = body_.jmp32();
    body_.patch_rel32(at, pop_at_);
  }
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
  /* V enters the block when the exit stores the flag halves or when a branch
   * tests it (VS/VC/GE/LT/GT/LE); nothing else can observe it. */
  v_read_ = false;
  for (uint32_t i = 0; i < n; ++i)
    if (in((int)i).op == IR_BR && cond_uses_v(in((int)i).cond)) v_read_ = true;
  v_loaded_ = exit_flags_ || v_read_ || blk_.uses_mem;
  /* A branch is the block's way out, so it can only be the last instruction. */
  for (uint32_t i = 0; i + 1 < n; ++i)
    if (in((int)i).op == IR_BR) return false;

  for (uint32_t i = 0; i < n; ++i) {
    if (!jit_x64_accepts(in((int)i))) return false;
    if (!op((int)i, lv[i], shape_of(in((int)i)))) return false;
  }
  /* Every guest register the body still holds in a host register has to reach
   * the shadow before the exit paths read it (a register branch reads R(b)
   * there, and so does the dispatcher). */
  for (int g = 0; g < kGuestRegs; ++g)
    if (host_of_[g] >= 0 && dirty_[g]) spill(g);
  if (!exit_path()) return false;

  /* The prologue is emitted after the body: only then does it know which
   * callee-saved registers the allocator reached for, and the body can be
   * discarded wholesale if an instruction turns out not to be emittable. */
  Buf code;
  std::swap(code.w, body_.w);
  body_.w.clear();
  prologue();
  body_.w.insert(body_.w.end(), code.w.begin(), code.w.end());

  out->assign(body_.w.begin(), body_.w.end());
  meta->entry_flags = entry_flags_ ? 1 : 0;
  meta->exit_flags = exit_flags_ ? 1 : 0;
  meta->uses_mem = blk_.uses_mem ? 1 : 0;
  meta->br = 0;
  if (blk_.n && blk_.in[blk_.n - 1].op == IR_BR)
    meta->br = blk_.in[blk_.n - 1].cond == 0xEu ? 1 : 2;
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

}  // namespace brain::arm926
