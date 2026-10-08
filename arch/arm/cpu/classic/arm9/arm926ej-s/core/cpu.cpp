/* brain-emu: ARM926EJ-S core -- lifecycle, the core half of the exception
 * model, the run loop, and the ISA environment.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * What lives here and not in the ISA layer (arch/arm/isa/armv5/): everything
 * that is the ARM926EJ-S rather than the ARMv5TEJ architecture.  The register
 * banking, the PSR semantics, the instruction semantics and the architectural
 * half of exception entry are the ISA's; taking a pending exception, writing
 * FSR/FAR into CP15, the cycle/statistics accounting, clearing the halt state
 * and notifying the JIT are this core's.  The run loop (budgets, halting,
 * stop requests, interrupt lines) is this core's, and so is `env` - the
 * IsaEnv table the ISA engine calls through for memory, translation, the
 * coprocessor and exception entry.
 *
 * Behaviour sources:
 *   - ARM DDI 0100I A2.6 (exception model; LR values are per tables in
 *     A2.6.x for ARM state and A6.1.2/table A6-1 for Thumb state),
 *   - ARM DDI 0198E 2.2/2.3 (CP15 register file and reset values),
 *   - ARM DDI 0198E 3.x (MMU, fault status encoding).
 */
#include "arch/arm/cpu/classic/arm9/arm926ej-s/core/cpu.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "arch/arm/isa/armv5/ops.h"          /* CycleModel: the exception charge */
#include "arch/arm/cpu/classic/arm9/arm926ej-s/jit/jit.h"

namespace brain::arm926 {

using namespace brain::arm::isa::armv5;

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
  bank_idx = bank_for_mode(MODE_SVC);

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

  /* The core-maintained behaviour policy the ISA engine reads (IsaState):
   * the strictness options come from the configuration, the L4 bit from
   * CP15 c1 (sync_cp15_caches, below, mirrors it). */
  jazelle_trap = cfg.jazelle_trap ? 1u : 0u;
  strict_unpredictable = cfg.strict_unpredictable ? 1u : 0u;
  l4 = 0;

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

/* The core half of exception entry: the architectural half (banking, SPSR,
 * LR, CPSR mode/I/F/E/T, PC := vector) is the ISA's (armv5::enter_exception);
 * what is added here is the ARM926EJ-S's own accounting - the E bit comes
 * from SCTLR.B, the exception count and the cycle charge go into the stats
 * the C API reports, the halt state is cleared, and the JIT is told so it
 * drops any block built under the pre-exception environment. */
void Cpu::enter_exception(u32 e, u32 vector, u32 mode, u32 lr) {
  const u32 e_bit = (cp15.c[C1_CTRL] & SCTLR_B) ? u32(CPSR_E) : 0u;
  armv5::enter_exception(*this, e, vector, mode, lr, e_bit);
  halted = 0;
  cycles += CycleModel::kExceptionCycles;
  if (e < EXC_COUNT) stats.exc[e]++;
  if (jit) jit->on_exception();
}

/* An exception raised by the instruction at `cur`: the abort status registers are
 * updated and R14 gets the instruction-relative offset from the ISA's table
 * (armv5::exception_lr_offset). */
void Cpu::take_exception(u32 e) {
  if (e == EXC_DABORT) {
    cp15.c[C5_FSR] = exc_fsr;
    cp15.c[C6_FAR] = exc_far;
  } else if (e == EXC_PABORT) {
    cp15.c[C5_IFSR] = exc_fsr;   /* FAR is not updated by prefetch aborts */
  }
  const u32 base = (cp15.c[C1_CTRL] & SCTLR_V) ? VEC_HIGH_BASE : 0u;
  enter_exception(e, base + armv5::vector_offset(e), armv5::exception_mode(e),
                  cur + armv5::exception_lr_offset(e, thumb != 0));
}

/* An interrupt (IRQ/FIQ) taken at the instruction boundary in front of `npc`:
 * "R14_irq = address of next instruction to be executed + 4" (A2.6.8) - so the
 * handler's `SUBS PC, R14, #4` lands on `npc`, the instruction that has not run
 * yet, and the guest resumes exactly where it was going to.  Both states use the
 * same +4: the two-byte Thumb encoding changes where the *next* instruction is,
 * not the delta the return instruction assumes. */
void Cpu::take_interrupt(u32 e) {
  const u32 base = (cp15.c[C1_CTRL] & SCTLR_V) ? VEC_HIGH_BASE : 0u;
  enter_exception(e, base + armv5::vector_offset(e), armv5::exception_mode(e), npc + 4u);
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

/* ---------------------------------------------------------- run loop -- */

void Cpu::step_one() {
  if (BRAIN_UNLIKELY(check_interrupts())) return;
  /* No fetch here: `cur` is only advanced inside the execution step, so a
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

/* ------------------------------------------- the ISA environment (env) --
 * The ISA engine (arch/arm/isa/armv5/interp*.cpp) calls through `env` for
 * everything that is not the instruction set: translated memory, the
 * instruction fetch, the coprocessor, taking a pending exception, JIT
 * notification and the cache sync.  The trampolines adapt the engine's
 * ctx-passing callbacks to Cpu methods; init_env wires them once, in the
 * constructor.  The memory trampolines are not a slowdown by construction:
 * each one is a tail call into the same inline fast path the interpreter
 * used to call directly. */

static u32 env_mem_read(void* ctx, u32 addr, u32 size, u32 flags) {
  return static_cast<Cpu*>(ctx)->mem_read(addr, size, flags);
}
static void env_mem_write(void* ctx, u32 addr, u32 val, u32 size, u32 flags) {
  static_cast<Cpu*>(ctx)->mem_write(addr, val, size, flags);
}
static u32 env_fetch16(void* ctx, u32 addr) {
  return static_cast<Cpu*>(ctx)->fetch16(addr);
}
static u32 env_fetch32(void* ctx, u32 addr) {
  return static_cast<Cpu*>(ctx)->fetch32(addr);
}
static bool env_prepare_fetch(void* ctx) {
  return static_cast<Cpu*>(ctx)->prepare_fetch();
}
static void env_take_exception(void* ctx, u32 e) {
  static_cast<Cpu*>(ctx)->take_exception(e);
}
static void env_notify_jit(void* ctx, const char* why) {
  Cpu* c = static_cast<Cpu*>(ctx);
  if (c->jit) c->jit->on_env_change(why);
}
static void env_sync_caches(void* ctx) {
  static_cast<Cpu*>(ctx)->sync_cp15_caches();
}
static bool env_coproc_present(void* ctx, u32 cp, u32 opc1, bool second) {
  return static_cast<Cpu*>(ctx)->coproc_present(cp, opc1, second);
}
static void env_coproc_write(void* ctx, u32 cp, u32 crn, u32 opc1, u32 crm,
                             u32 opc2, u32 val) {
  static_cast<Cpu*>(ctx)->coproc_write(cp, crn, opc1, crm, opc2, val);
}
static u32 env_coproc_read(void* ctx, u32 cp, u32 crn, u32 opc1, u32 crm,
                           u32 opc2) {
  return static_cast<Cpu*>(ctx)->coproc_read(cp, crn, opc1, crm, opc2);
}

void Cpu::init_env() {
  env.ctx = this;
  env.stats = &stats;
  env.mem_read = env_mem_read;
  env.mem_write = env_mem_write;
  env.fetch16 = env_fetch16;
  env.fetch32 = env_fetch32;
  env.prepare_fetch = env_prepare_fetch;
  env.coproc_present = env_coproc_present;
  env.coproc_write = env_coproc_write;
  env.coproc_read = env_coproc_read;
  env.take_exception = env_take_exception;
  env.notify_jit = env_notify_jit;
  env.sync_caches = env_sync_caches;
}

/* The engine's coprocessor view of CP15: the ARM926EJ-S implements exactly
 * CP15, opc1 0 and 1, and neither of the MCR2/MRC2 aliases - which is what
 * the engine's UNDEF check asks about.  (cp15_write/cp15_read keep their own
 * User-mode guard; the engine checks privilege before asking, so it never
 * fires from the engine's path.) */
bool Cpu::coproc_present(u32 cp, u32 opc1, bool second) const {
  return cp == 15 && opc1 <= 1 && !second;
}
void Cpu::coproc_write(u32 cp, u32 crn, u32 opc1, u32 crm, u32 opc2, u32 val) {
  (void)cp;
  cp15_write(crn, opc1, val, crm, opc2);
}
u32 Cpu::coproc_read(u32 cp, u32 crn, u32 opc1, u32 crm, u32 opc2) {
  (void)cp;
  return cp15_read(crn, opc1, crm, opc2);
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
