/* brain-emu: ARM926EJ-S core -- lifecycle, mode banking, exceptions, run loop.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Behaviour sources:
 *   - ARM DDI 0100I A2.6 (exception model; LR values are per tables in
 *     A2.6.x for ARM state and A6.1.2/table A6-1 for Thumb state),
 *   - ARM DDI 0198E 2.2/2.3 (CP15 register file and reset values),
 *   - ARM DDI 0198E 3.x (MMU, fault status encoding).
 */
#include "machines/arm926/cpu/cpu.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "machines/arm926/cpu/ops.h"
#include "machines/arm926/jit/jit.h"

namespace brain::arm926 {

/* ---------------------------------------------------------------- names -- */

const char* cond_name(u32 c) {
  static const char* n[16] = {"EQ","NE","CS","CC","MI","PL","VS","VC","HI","LS",
                              "GE","LT","GT","LE","AL","NV"};
  return n[c & 15];
}
const char* mode_name(u32 m) {
  switch (m & CPSR_MODE_M) {
    case MODE_USR: return "USR";
    case MODE_FIQ: return "FIQ";
    case MODE_IRQ: return "IRQ";
    case MODE_SVC: return "SVC";
    case MODE_ABT: return "ABT";
    case MODE_UND: return "UND";
    case MODE_SYS: return "SYS";
    default: return "???";
  }
}
const char* exc_name(u32 e) {
  static const char* n[EXC_COUNT + 1] = {"none", "reset", "undef", "swi",
                                        "pabort", "dabort", "irq", "fiq"};
  return e <= EXC_COUNT ? n[e] : "?";
}

static u32 bank_for_mode_static(u32 mode) {
  switch (mode & CPSR_MODE_M) {
    case MODE_FIQ: return BANK_FIQ;
    case MODE_IRQ: return BANK_IRQ;
    case MODE_SVC: return BANK_SVC;
    case MODE_ABT: return BANK_ABT;
    case MODE_UND: return BANK_UND;
    default: return BANK_USR;   /* USR and SYS share the user bank */
  }
}

/* ------------------------------------------------------------- banking -- */

u32 Cpu::bank_for_mode(u32 mode) const { return bank_for_mode_static(mode); }

/* Which registers a mode actually banks (TRM 4.1.1, "register bank"): R13 and
 * R14 in every mode, plus R8-R12 in FIQ only.  R0-R7 - and R8-R12 outside FIQ -
 * name the same physical registers in all modes, so a mode change must leave
 * them untouched.  Copying the whole bank instead made any exception wipe r0-r7
 * of the faulting thread, which is why this split is spelled out.  The shared
 * view of R8-R12 lives in the user bank, so leaving FIQ restores it. */
static bool bank_is_fiq(u32 idx) { return idx == BANK_FIQ; }

void Cpu::flush_bank() {
  Bank& b = bank[bank_idx];
  b.r[13] = regs[13];
  b.r[14] = regs[14];
  Bank& shared = bank_is_fiq(bank_idx) ? b : bank[BANK_USR];
  for (u32 i = 8; i < 13; ++i) shared.r[i] = regs[i];
  /* SPSR_<mode> is deliberately *not* written here.  It is not a mirror of the
   * CPSR: it holds the value an exception saved on entry, and MRS/MSR see it as
   * bank[bank_idx].spsr.  Saving the current CPSR on every mode change made an
   * IRQ taken inside a Supervisor handler (or a detour through System mode)
   * overwrite the SPSR that handler was about to return with. */
}

void Cpu::load_bank(u32 idx) {
  bank_idx = idx;
  const Bank& b = bank[idx];
  regs[13] = b.r[13];
  regs[14] = b.r[14];
  const Bank& shared = bank_is_fiq(idx) ? b : bank[BANK_USR];
  for (u32 i = 8; i < 13; ++i) regs[i] = shared.r[i];
}

/* Install a mode written by MSR / LDM-into-R15 / the debugger, banking the
 * registers that actually change.  The *live bank*, not the CPSR field, decides
 * whether a swap is needed: an MSR-style caller has usually written the new mode
 * into CPSR before calling here, and comparing against CPSR then makes the
 * change look like a no-op and silently keeps the old R13/R14 - which is how a
 * supervisor stack used to survive a detour through System mode. */
void Cpu::switch_mode(u32 new_mode) {
  new_mode &= CPSR_MODE_M;
  cpsr = (cpsr & ~CPSR_MODE_M) | new_mode;
  priv = (new_mode != MODE_USR);
  const u32 idx = bank_for_mode_static(new_mode);
  if (idx == bank_idx) return;   /* USR<->SYS share a bank: nothing to move. */
  flush_bank();
  load_bank(idx);
}

/* ----------------------------------------------------------- R15 writes -- */

void Cpu::set_reg(u32 n, u32 val) {
  if (n == 15) {
    if (thumb) {
      npc = val & ~1u;             /* bit[0] selects the state, as for BX     */
      if (!(val & 1u)) { thumb = 0; cpsr &= ~(CPSR_T | CPSR_J); }
    } else {
      npc = val & ~3u;             /* bits[1:0] ignored in ARM state          */
    }
    /* cur/regs[15] are refreshed by the next fetch: see Cpu::fetch_and_execute */
    return;
  }
  regs[n] = val;
  /* Keep the bank of the mode we are in in step, or the next mode change (an
   * exception, an MSR) would restore the stale value over this one.  Only
   * R13/R14 and, in FIQ, R8-R12 are banked - see Cpu::flush_bank(). */
  if (n >= 13 || (bank_idx == BANK_FIQ && n >= 8)) bank[bank_idx].r[n] = val;
}

/* -------------------------------------------------------------- reset -- */

bool Cpu::init(const CpuConfig& c) {
  /* Standalone use: this object owns its address space.  A second init on the
   * same object rebuilds it rather than stacking a second RAM window on top of
   * the first (which the bus would refuse anyway). */
  if (bus_impl.region_count()) bus_impl.reset();
  bus = &bus_impl;
  if (c.ram_size && !bus_impl.map_ram(c.ram_base, c.ram_size,
                                      c.ram_name ? c.ram_name : "ram"))
    return false;
  return attach(bus_impl, c);
}

void Cpu::on_bus_store(void* user, u32 addr, u32 size) {
  static_cast<Cpu*>(user)->invalidate_code(addr, size);
}

bool Cpu::attach(Bus& external, const CpuConfig& c) {
  cfg = c;
  bus = &external;
  /* Self-modifying code: the bus tells us when a store lands in a page that
   * holds translated code, and we drop the blocks.  Installed even when the JIT
   * is off - it is one null check on the store path, and it means enabling the
   * JIT later cannot forget to hook it up. */
  bus->set_store_observer(&Cpu::on_bus_store, this);

  u32 n = 1u << (cfg.tlb_bits ? cfg.tlb_bits : 11);
  delete[] tlb_store;               /* a second init replaces the old table */
  tlb_store = new TlbEntry[2 * n];
  for (u32 i = 0; i < 2 * n; ++i) { tlb_store[i].vpn = TLB_FREE; tlb_store[i].host = nullptr; }
  tlb_d = tlb_store;
  tlb_i = tlb_store + n;
  tlb_mask = n - 1;

  ram = bus->ram_host();
  ram_gbase = bus->ram_base();
  ram_mask = bus->ram_mask();
  reset();
  if (cfg.use_jit && !jit_enable(true)) {
    /* No code generator for this host (or the cache could not be mapped):
     * degrade to the interpreter rather than fail the caller.  Semantics and
     * the cycle model are identical; only speed differs. */
    cfg.use_jit = false;
    if (cfg.trace) std::fprintf(stderr, "[cpu] JIT unavailable: interpreting\n");
  }
  return true;
}

void Cpu::reset() {
  memset(regs, 0, sizeof(regs));
  memset(bank, 0, sizeof(bank));
  memset(cp15.c, 0, sizeof(cp15.c));
  bank_idx = bank_for_mode_static(MODE_SVC);

  /* TRM table 2-5: implementer 0x41, arch 0x6 (ARMv5TEJ), part 0x926, rev 5 */
  cp15.c[C0_IDCODE] = ARM926EJS_MIDR;
  /* TRM figure 2-2: S=1 (separate I/D caches), 16KiB 4-way, 32B lines */
  cp15.c[C0_CACHE_TYPE] = ARM926EJS_CTYPE;
  /* DDI 0198: the Control Register resets to 0x00050078 on the ARM926EJ-S
   * (bits 16 and 18 are SBO, 17 SBZ, [31:19] SBZ, and the low byte carries
   * the C/B/S/F enables).  MRC of c1 must read that back, so it is stored
   * rather than assumed; the only bits with a function in this model are
   * M(0), A(1), C(2), I(12), RR(13) and X(19) -- A is 0 at reset, which is
   * what makes unaligned ARM data processing legal, and M is 0, so the MMU
   * and its AP/XN checks only start applying after an explicit enable. */
  cp15.c[C1_CTRL] = 0x00050078u | (cfg.high_vectors ? SCTLR_V : 0u);
  cp15.c[C10_TS] = 0x00570000u;
  cp15.c[C15_CFR] = 0x00008001u;

  cpsr = MODE_SVC | CPSR_I | CPSR_F | (cfg.big_endian_data ? u32(CPSR_E) : 0u);
  if (cfg.big_endian_data) cp15.c[C1_CTRL] |= SCTLR_B;
  vflag = 0;
  thumb = 0;
  halted = 0;
  exit_req = 0;
  cycles = 0;
  insns = 0;
  exc = exectrap = exc_fsr = exc_far = 0;
  jit_flags = 0;
  irq_line = fiq_line = 0;
  syscallno = sys_ret = exit_code = 0;
  syscall_count = 0;
  running = 0;
  trace_en = cfg.trace ? 1 : 0;
  stats = Stats{};

  u32 vec = (cp15.c[C1_CTRL] & SCTLR_V) ? VEC_HIGH_BASE : 0u;
  npc = vec;
  cur = vec;
  insn = 0;

  mmu_on = 0;
  align_chk = 0;
  be_data = cfg.big_endian_data ? 1 : 0;
  priv = 1;
  smc_watch = 0;
  trans_gen = 1;
  ram_fast = (ram && ram_mask) ? 1u : 0u;
  sync_cp15_caches();
  tlb_flush_all();
  if (jit) jit->flush_all("reset");
}

Cpu::~Cpu() {
  jit_enable(false);      /* the code cache owns an mmap'd arena            */
  delete[] tlb_store;
}

bool Cpu::jit_enable(bool on) {
  if (!on) { delete jit; jit = nullptr; return true; }
  if (jit) return true;
  jit = new Jit(this);
  if (!jit->init()) { delete jit; jit = nullptr; return false; }
  ram_fast = (!mmu_on && ram && ram_mask) ? 1u : 0u;
  return true;
}

/* ---------------------------------------------------------- exceptions -- */

/* R14 at exception entry.  There are two families here and they are specified
 * against different addresses, which is the single easiest thing to get wrong in
 * an exception model (DDI 0100I A2.6.3-A2.6.10 in ARM state, A6.1.2 with table
 * A6-1 in Thumb state):
 *
 *  - an exception *raised by an instruction* is reported against that
 *    instruction's address, which is `cur` while the trap is being taken:
 *
 *                     ARM  Thumb   handler return
 *      undefined       +4    +2    `MOVS PC, R14` -> the next instruction
 *      SWI             +4    +2
 *      prefetch abort  +4    +4    `SUBS PC, R14, #4` -> retry the fetch
 *      data abort      +8    +8    `SUBS PC, R14, #8` -> re-execute
 *
 *  - an interrupt (IRQ/FIQ) is taken *between* instructions, at an instruction
 *    boundary, and the ARM ARM specifies it against the instruction that has not
 *    run yet: "R14_irq = address of next instruction to be executed + 4"
 *    (A2.6.8).  That instruction is `npc` here, and the +4 is state-independent -
 *    a Thumb guest uses the same `SUBS PC, R14, #4` return, so what moves is the
 *    base address, not the offset (see take_interrupt()).
 *
 * The distinction is not academic: for a fall-through the two readings agree
 * (npc + 4 == cur + 8 in ARM state), but when the last retired instruction was a
 * *taken branch* they no longer do, and a handler built on the wrong one resumes
 * at an address the guest never intended - silently skipping or repeating one
 * instruction, in the worst case at the top of a loop.
 */
u32 Cpu::exception_lr_offset(u32 e) const {
  if (thumb) {
    switch (e) {
      case EXC_UNDEF:  return 2;   /* the next halfword                     */
      case EXC_SWI:    return 2;
      case EXC_PABORT: return 4;   /* aborted + 4 = retry the fetch          */
      case EXC_DABORT: return 8;   /* aborted + 8 = re-execute               */
      default: return 0;           /* interrupts do not use this table       */
    }
  }
  switch (e) {
    case EXC_UNDEF:  return 4;     /* the next instruction                  */
    case EXC_SWI:    return 4;
    case EXC_PABORT: return 4;     /* aborted + 4                           */
    case EXC_DABORT: return 8;     /* aborted + 8                           */
    default: return 0;             /* interrupts do not use this table       */
  }
}

static u32 vector_offset(u32 e) {
  switch (e) {
    case EXC_RESET: return VEC_RESET;
    case EXC_UNDEF: return VEC_UNDEF;
    case EXC_SWI: return VEC_SWI;
    case EXC_PABORT: return VEC_PABORT;
    case EXC_DABORT: return VEC_DABORT;
    case EXC_IRQ: return VEC_IRQ;
    case EXC_FIQ: return VEC_FIQ;
    default: return VEC_RSVD;
  }
}

void Cpu::enter_exception(u32 e, u32 vector, u32 mode, u32 lr) {
  const u32 old_cpsr = cpsr;
  flush_bank();

  const u32 idx = bank_for_mode_static(mode);
  Bank& nb = bank[idx];
  nb.spsr = old_cpsr;
  nb.r[14] = lr;
  regs[13] = nb.r[13];
  regs[14] = nb.r[14];
  const Bank& shared = (idx == BANK_FIQ) ? nb : bank[BANK_USR];
  for (u32 i = 8; i < 13; ++i) regs[i] = shared.r[i];
  bank_idx = idx;

  /* Keep N,Z,C,V,Q and (ARMv5) the A bit.  Set I, and F for FIQ/reset.
   * E <- CP15 c1[B] (BE8 config).  Force ARM state (T=0, J=0). */
  u32 keep = old_cpsr & (CPSR_NVQC | CPSR_A);
  u32 e_bit = (cp15.c[C1_CTRL] & SCTLR_B) ? u32(CPSR_E) : 0u;
  u32 f_bit = (e == EXC_FIQ || e == EXC_RESET) ? CPSR_F : (old_cpsr & CPSR_F);
  cpsr = keep | e_bit | f_bit | CPSR_I | mode;
  thumb = 0;
  vflag = (cpsr >> 28) & 1;
  priv = 1;

  /* The handler is the *next* instruction to fetch; cur/regs[15] are primed
   * by the fetch, so LR keeps pointing at the faulting instruction. */
  npc = vector;
  insn = 0;
  halted = 0;
  exc = 0;
  exectrap = 0;
  cycles += CycleModel::kExceptionCycles;
  if (e < EXC_COUNT) stats.exc[e]++;
  if (jit) jit->on_exception();
}

/* Mode and vector for an exception - the two tables of DDI 0100I A2.6 and the
 * layout of the vector page.  Shared by the traps and the interrupts. */
static u32 exception_mode(u32 e) {
  switch (e) {
    case EXC_UNDEF: return MODE_UND;
    case EXC_IRQ:   return MODE_IRQ;
    case EXC_FIQ:   return MODE_FIQ;
    case EXC_PABORT:
    case EXC_DABORT: return MODE_ABT;
    default: return MODE_SVC;   /* SWI, reset */
  }
}

/* An exception raised by the instruction at `cur`: the abort status registers are
 * updated and R14 gets the instruction-relative offset from the table above. */
void Cpu::take_exception(u32 e) {
  if (e == EXC_DABORT) {
    cp15.c[C5_FSR] = exc_fsr;
    cp15.c[C6_FAR] = exc_far;
  } else if (e == EXC_PABORT) {
    cp15.c[C5_IFSR] = exc_fsr;   /* FAR is not updated by prefetch aborts */
  }
  const u32 base = (cp15.c[C1_CTRL] & SCTLR_V) ? VEC_HIGH_BASE : 0u;
  enter_exception(e, base + vector_offset(e), exception_mode(e),
                  cur + exception_lr_offset(e));
}

/* An interrupt (IRQ/FIQ) taken at the instruction boundary in front of `npc`:
 * "R14_irq = address of next instruction to be executed + 4" (A2.6.8) - so the
 * handler's `SUBS PC, R14, #4` lands on `npc`, the instruction that has not run
 * yet, and the guest resumes exactly where it was going to.  Both states use the
 * same +4: the two-byte Thumb encoding changes where the *next* instruction is,
 * not the delta the return instruction assumes. */
void Cpu::take_interrupt(u32 e) {
  const u32 base = (cp15.c[C1_CTRL] & SCTLR_V) ? VEC_HIGH_BASE : 0u;
  enter_exception(e, base + vector_offset(e), exception_mode(e), npc + 4u);
}

void Cpu::raise_undef(u32 i) { insn = i; exc = EXC_UNDEF; exectrap = EXC_UNDEF; }
void Cpu::raise_swi(u32 imm) { (void)imm; exc = EXC_SWI; exectrap = EXC_SWI; }
void Cpu::raise_dabort(u32 fs, u32 va) {
  exc = EXC_DABORT; exectrap = EXC_DABORT; exc_fsr = fs; exc_far = va;
  stats.page_faults++;
}
void Cpu::raise_pabort(u32 fs) {
  exc = EXC_PABORT; exectrap = EXC_PABORT; exc_fsr = fs; exc_far = cur;
  stats.page_faults++;
}

bool Cpu::check_interrupts() {
  if (BRAIN_UNLIKELY(fiq_line && !(cpsr & CPSR_F))) { take_interrupt(EXC_FIQ); return true; }
  if (BRAIN_UNLIKELY(irq_line && !(cpsr & CPSR_I))) {
    halted = 0;
    take_interrupt(EXC_IRQ);
    return true;
  }
  return false;
}

/* -------------------------------------------------------------- run loop -- */

void Cpu::step_one() {
  if (BRAIN_UNLIKELY(check_interrupts())) return;
  /* No fetch here: `cur` is only advanced inside Cpu::fetch_and_execute, so a
   * prefetch check at this point would look at the instruction that has just
   * been executed.  A guest whose abort vector is not itself mapped then retook
   * the same abort for ever - `cur` never moved to the vector, nothing retired,
   * and no bounded run could terminate. */
  fetch_and_execute();
  /* Retired: print it before deciding whether it raised a trap, so the line
   * that caused an exception is visible too. */
  if (BRAIN_UNLIKELY(trace_en)) trace_instruction();
  if (BRAIN_UNLIKELY(exectrap)) {
    u32 e = exectrap;
    take_exception(e);
  }
}

/* The one run loop, and the only place `halted` is interpreted.
 *
 * `insn_stop`/`cyc_stop` are absolute *targets* on the two counters (the bounds
 * arrive as deltas), and `~0ull` means "no bound on this axis".  A halted CPU
 * still charges cycles: that is what a guest waiting for an interrupt does with
 * the time, and it is what lets a caller that bound the run in cycles get
 * control back to tick the peripherals that will raise one.  Nothing but
 * `exit_req` (a stop request: a device, a debugger) ends the loop early - so a
 * caller can tell "the CPU stopped" from "the budget ran out" by asking. */
void Cpu::run_until(u64 max_insns, u64 max_cycles) {
  const u64 insn_stop = max_insns ? insns + max_insns : ~0ull;
  const u64 cyc_stop = max_cycles ? cycles + max_cycles : ~0ull;
  running = 1;
  exit_req = 0;
  if (jit && cfg.use_jit) {
    jit->run_until(max_insns, max_cycles);
    running = 0;
    return;
  }
  while (insns < insn_stop && cycles < cyc_stop && !exit_req) {
    if (BRAIN_UNLIKELY(halted)) {
      if (!(irq_line && !(cpsr & CPSR_I)) && !(fiq_line && !(cpsr & CPSR_F))) {
        cycles += 1;
        continue;
      }
      halted = 0;
    }
    step_one();
  }
  running = 0;
}

/* ------------------------------------------------------- bus-level I/O -- */

/* Physical access through whichever bus this CPU is attached to: the machine's
 * (the usual case) or the private one `init` made for standalone use.  The JIT
 * slow path, the loaders and the standalone tests all come through here, and
 * `bus` is the only correct target for any of them. */
u32 Cpu::phys_read(u32 a, u32 size) {
  u32 acc = 0;
  for (u32 i = 0; i < size; ++i) {
    u8 b = 0;
    if (bus) bus->read_byte(a + i, &b);
    acc |= (u32)b << (i * 8);
  }
  return acc;
}

void Cpu::phys_write(u32 a, u32 val, u32 size) {
  for (u32 i = 0; i < size; ++i)
    if (bus) bus->write_byte(a + i, (u8)(val >> (i * 8)));
  if (jit) invalidate_code(a, size);
}

bool Cpu::load_image(u32 a, const void* p, size_t n) {
  const bool ok = bus && bus->write_bytes(a, p, n);
  if (jit) invalidate_code(a, (u32)n);
  return ok;
}

void Cpu::invalidate_code(u32 addr, u32 size) {
  if (jit) jit->invalidate_range(addr, size);
}

/* ------------------------------------------------------------ tracing -- */

void Cpu::trace(const char* what) { fprintf(stderr, "[cpu] %s\n", what); }

void Cpu::trace_instruction() {
  char buf[96];
  if (thumb) format_thumb_insn(insn, buf, sizeof(buf));
  else format_arm_insn(insn, buf, sizeof(buf));
  fprintf(stderr, "%c %08x:%08x  %-44s r0=%08x r1=%08x r13=%08x r14=%08x %c%c%c%c%s\n",
          thumb ? 'T' : 'A', cur, insn, buf, regs[0], regs[1], regs[13], regs[14],
          (cpsr & CPSR_N) ? 'N' : '-', (cpsr & CPSR_Z) ? 'Z' : '-',
          (cpsr & CPSR_C) ? 'C' : '-', vflag ? 'V' : '-',
          (cpsr & CPSR_Q) ? " Q" : "");
}

/* --------------------------------------------------- static JIT helpers -- */

void Cpu::h_raise(Cpu* c, u32 e) {
  c->stats.helper_calls++;
  c->exc = e;
  c->exectrap = e;
}

u32 Cpu::h_read32(Cpu* c, u32 a) { c->stats.helper_calls++; return c->mem_read(a, 4, MF_SZ32); }
u32 Cpu::h_read16(Cpu* c, u32 a) { c->stats.helper_calls++; return c->mem_read(a, 2, MF_SZ16); }
u32 Cpu::h_read8(Cpu* c, u32 a)  { c->stats.helper_calls++; return c->mem_read(a, 1, MF_SZ8); }
void Cpu::h_write32(Cpu* c, u32 a, u32 vflag) { c->stats.helper_calls++; c->mem_write(a, vflag, 4, MF_SZ32 | MF_WRITE); }
void Cpu::h_write16(Cpu* c, u32 a, u32 vflag) { c->stats.helper_calls++; c->mem_write(a, vflag, 2, MF_SZ16 | MF_WRITE); }
void Cpu::h_write8(Cpu* c, u32 a, u32 vflag)  { c->stats.helper_calls++; c->mem_write(a, vflag, 1, MF_SZ8 | MF_WRITE); }

/* Out-of-line escape hatch: the JIT translates everything it understands and
 * executes the rest with the interpreter, then returns to the dispatcher. */
void Cpu::h_exec_arm(Cpu* c, u32 i) { c->stats.helper_calls++; c->exec_arm(i); }
void Cpu::h_exec_thumb(Cpu* c, u32 i) { c->stats.helper_calls++; c->exec_thumb(i); }

void Cpu::h_branch_exchange(Cpu* c, u32 target) {
  c->stats.helper_calls++;
  if (target & 1u) {
    c->thumb = 1;
    c->cpsr |= CPSR_T;
    c->cpsr &= ~CPSR_J;
    c->npc = target & ~1u;
  } else if ((target & 3u) == 2 && c->cfg.jazelle_trap) {
    /* bit0 clear, bit1 set: Jazelle entry (BXJ semantics). */
    c->cpsr |= CPSR_J;
    c->npc = target & ~3u;
  } else {
    c->thumb = 0;
    c->cpsr &= ~(CPSR_T | CPSR_J);
    c->npc = target & ~3u;
  }
}

void Cpu::h_check_gen(Cpu* c, u32 gen) {
  /* A block recorded `gen` at translation time; if the environment changed
   * since, the block is stale and must not run any further. */
  (void)gen;
  if (c->jit) c->jit->request_retranslate();
}

/* ----------------------------------------------------------- MSR / MRS -- */

static u32 msr_mask_to_cpsr(u32 mask) {
  u32 m = 0;
  if (mask & MSR_F) m |= CPSR_CTRL_M;
  if (mask & MSR_S) m |= CPSR_MODE_M;
  if (mask & MSR_X) m |= CPSR_Q;
  if (mask & MSR_C) m |= CPSR_FLAGS;
  return m & CPSR_WRITABLE;
}

void Cpu::h_msr_cpsr(Cpu* c, u32 mask, u32 val) {
  c->stats.helper_calls++;
  const u32 m = msr_mask_to_cpsr(mask);
  const u32 old = c->cpsr;
  u32 nv = (old & ~m) | (val & m);
  /* T/J are not writable through MSR on ARMv5 (UNPREDICTABLE); keep them.  */
  nv = (nv & ~(CPSR_T | CPSR_J)) | (old & (CPSR_T | CPSR_J));
  c->cpsr = nv;
  c->vflag = (nv >> 28) & 1;
  if ((old ^ nv) & CPSR_MODE_M) c->switch_mode(nv & CPSR_MODE_M);
  if ((old ^ nv) & (CPSR_E | CPSR_A)) c->sync_cp15_caches();
  if ((old ^ nv) & CPSR_MODE_M) c->on_translation_env_changed("msr-mode");
}

void Cpu::h_msr_spsr(Cpu* c, u32 mask, u32 val) {
  c->stats.helper_calls++;
  const u32 m = msr_mask_to_cpsr(mask);
  c->bank[c->bank_idx].spsr = (c->bank[c->bank_idx].spsr & ~m) | (val & m);
}

}  // namespace brain::arm926
