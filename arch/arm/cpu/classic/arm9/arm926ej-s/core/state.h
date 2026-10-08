/* brain-emu -- ARM926EJ-S core: the core-specific state, on top of the ISA.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This header is the ARM926EJ-S *core*.  The instruction set architecture it
 * executes lives one layer down, in arch/arm/isa/armv5/ (the ARMv5TEJ ISA:
 * decoders, execution semantics, PSR/flags, banking, the exception model),
 * and the execution state it runs in lives in arch/arm/execution/aarch32/
 * (the pipeline step, the ARM/Thumb dispatch).  What is left here is exactly
 * what makes this core this core:
 *
 *   - CP15, the system control coprocessor: its register shadow, the SCTLR
 *     bits, the ID/cache-type constants, and the write side effects (TLB
 *     ops, cache ops, the translation-environment change notification);
 *   - the MMU: the two-level ARMv5 translation, the TLB, the domain/AP
 *     checks, the fetch translation cache;
 *   - the configuration caches derived from CP15 (mmu_on, align_chk, be_data,
 *     ttb, domain) and the identity RAM fast-path window;
 *   - the run loop: budgets, halting, stop requests, interrupt lines, the
 *     JIT dispatcher's control plane, and the trace;
 *   - the environment the ISA engine runs against: `env` (IsaEnv) is this
 *     core's answer to every callback the engine makes.
 *
 * Cpu inherits the architectural state (`struct Cpu : public IsaState`), so
 * the engine, this core's own code, the JIT helpers and the tests all operate
 * on one object with one register file: `cpu.regs`, `cpu.cpsr`, `cpu.exec_arm`
 * and friends resolve exactly as they always have, while the implementations
 * live in the ISA layer.  The engine entry points below are thin forwarders
 * into that layer (the JIT's out-of-line helpers call them, so a bailed-out
 * instruction and an interpreted one are the same code).
 *
 * Layout: the architectural hot fields (register file, pipeline view, CPSR +
 * V mirror, counters, pending exception) sit at the top of the base, the
 * bank array follows, and the core's own fields come after - the JIT
 * dispatcher copies the architectural part per block entry (by field name;
 * nothing is addressed by offset - generated code talks to the shadow buffer,
 * jit/abi.h).
 *
 * References: ARM DDI 0198E (ARM926EJ-S TRM); ARM DDI 0100I (ARM ARM v5).
 */
#ifndef BRAINEMU_CORE_STATE_H
#define BRAINEMU_CORE_STATE_H

#include <cstddef>
#include <cstring>

#include "brainemu/bus.h"
#include "brainemu/cpu.h"
#include "brainemu/types.h"

#include "arch/arm/execution/aarch32/step.h"
#include "arch/arm/isa/armv5/decode.h"
#include "arch/arm/isa/armv5/exceptions.h"
#include "arch/arm/isa/armv5/interp.h"
#include "arch/arm/isa/armv5/memory.h"
#include "arch/arm/isa/armv5/state.h"

namespace brain::arm926 {

namespace armv5 = brain::arm::isa::armv5;
namespace aarch32 = brain::arm::execution::aarch32;

/* BE32 (CP15 c1[B]) swaps the byte order inside a word: a halfword access
 * therefore sees its two bytes in the opposite order.  Keep the result to 16
 * bits - the shift-then-or form would otherwise leak the upper half. */
ALWAYS_INLINE u32 be32_swap16(u32 v) { return ((v >> 8) | (v << 8)) & 0xFFFFu; }

enum : u32 {
  C0_IDCODE = 0,   /* c0,0,0: ID code, table 2-5                            */
  C0_CACHE_TYPE,   /* c0,0,1                                                */
  C0_TCM_R,        /* c0,0,2 (ARM926EJ-S: no TCM, reads 0)                 */
  C0_TCM_A,        /* c0,0,7                                                */
  C1_CTRL,         /* c1,0,0: system control                               */
  C1_TEST,         /* c1,0,1: test (TI bit etc.)                           */
  C1_AUX,          /* c1,0,7: auxiliary control                            */
  C2_TTB,          /* c2,0,0: translation table base                       */
  C3_DACC,         /* c3,0,0: domain access control                        */
  C4_FMR,          /* c4: locked regions (not implemented, reads 0)         */
  C5_FSR,          /* c5,0,0: data fault status                            */
  C5_IFSR,         /* c5,0,1: instruction (prefetch) fault status          */
  C6_FAR,          /* c6,0,0: fault address                                */
  C7_OPS,          /* c7: cache/TLB ops (write only)                       */
  C8_TLB,          /* c8: TLB ops (write only)                             */
  C9_DC,           /* c9,1,0: D-cache lockdown / line size                 */
  C9_IC,           /* c9,1,1: I-cache lockdown                             */
  C10_TS,          /* c10,0,2: TLB structure (VR/LR)                      */
  C13_PROCID,      /* c13,0,0: FCSE PID, seven bits pre-shifted into [31:25] */
  C13_CTXID,       /* c13,0,1: context ID (32-bit, for trace tools)          */
  C15_CFR,         /* c15: test and debug, implementation-defined (2.3.15)   */
  CPCOUNT,
};

/* CP15 c1 bits (TRM 3-30). */
enum : u32 {
  /* DDI 0198E Table 2-11, Control Register c1.  Bits [6:3] are reserved-SBO
   * and [17] / [11:10] are SBZ, so they are not modelled as functions; [16]
   * and [18] are SBO.  The core has no branch-predict or write-protect enable
   * in this register, so nothing here pretends to be one. */
  SCTLR_M  = 1u << 0,  /* MMU enable                                     */
  SCTLR_A  = 1u << 1,  /* alignment fault checking on data addresses     */
  SCTLR_C  = 1u << 2,  /* DCache enable                                  */
  SCTLR_B  = 1u << 7,  /* endianness of data access: 0 = LE, 1 = BE      */
  SCTLR_S  = 1u << 8,  /* system protection (MPU regions)                */
  SCTLR_R  = 1u << 9,  /* ROM protection                                  */
  SCTLR_I  = 1u << 12, /* ICache enable                                  */
  SCTLR_V  = 1u << 13, /* high exception vectors (from VINITHI at reset) */
  SCTLR_RR = 1u << 14, /* round-robin cache replacement                  */
  SCTLR_L4 = 1u << 15, /* ARMv4 T-bit behaviour on loads to PC           */
  /* Bits [31:19] are reserved: a write should be zero but a read returns
   * whatever was written (the TRM recommends read-modify-write for future
   * compatibility), so they are neither modelled as functions nor masked.
   * There is no execute-never control anywhere on this core. */
  /* Read-as-set / write-ignored and read-as-zero fields of c1. */
  SCTLR_C1_SBO = (1u << 16) | (1u << 18) | 0x00000078u,
  SCTLR_C1_SBZ = (1u << 17) | 0x00000C00u,
};

/* ARM926EJ-S identification.  The ID code is TRM table 2-5: implementer 0x41,
 * variant 0, architecture 0x6 (ARMv5TEJ), part number 0x926, revision 5 --
 * that is, the r0p5 the TRM title describes. */
enum : u32 {
  ARM926EJS_MIDR = (0x41u << 24) | (0x0u << 20) | (0x6u << 16) | (0x926u << 4) | 5u,
  /* Cache Type Register, TRM table 2-10: Ctype b1110 (write-back, register 7
   * cleaning, lockdown format C), S = 1 (separate I/D caches), and per-cache
   * {size, assoc, M, len}.  Sizes are a *configuration* choice of the chip:
   * 16 KiB 4-way with 8-word lines, as used in the i.MX28. */
  ARM926EJS_CTYPE = (0xEu << 25) | (1u << 24) |
                    (0x5u << 18) | (0x2u << 15) | (0x2u << 12) |   /* DCache */
                    (0x5u << 6)  | (0x2u << 3)  | (0x2u << 0),    /* ICache */
  ARM926EJS_CF15  = 0x00008001u,   /* ARM926EJ-S specific config            */
};

enum : u32 { TLB_FREE = 0xFFFFFFFFu };

enum : u32 {
  TLB_VALID  = 1u << 0,
  TLB_HOST   = 1u << 1,   /* `host` is a valid direct mapping (RAM/ROM)     */
  TLB_READ   = 1u << 2,
  TLB_WRITE  = 1u << 3,
  TLB_PRIV   = 1u << 4,   /* privileged access only (AP denies user)        */
  TLB_DEVICE = 1u << 5,
  TLB_ROM    = 1u << 7,   /* host mapping not writable                      */
  TLB_SECT   = 1u << 8,   /* section-backed (1 MiB) => 4 KiB aliases shared */
};

struct TlbEntry {
  u32 vpn;     /* virtual page number, TLB_FREE when empty                 */
  u32 gpa;     /* guest physical page number                               */
  u8* host;    /* host pointer of the *start of the page*, or null         */
  u32 attr;    /* TLB_* bits                                                */
  u32 priv;    /* privilege the entry was created for (0 user, 1 priv)      */
  u32 gen;     /* cpu->trans_gen at fill time                               */
};
static_assert(sizeof(TlbEntry) == 32, "TlbEntry must be 32 bytes");

/* ================================================================= JIT === */

enum : u32 {
  JITF_HOST_NZC = 1u << 0,  /* guest N/Z/C live in the host flags register  */
};

/* ================================================================ config == */

struct CpuConfig {
  u32 ram_base = 0x00000000;
  u32 ram_size = 32u << 20;
  const char* ram_name = "ram";

  u32 tlb_bits = 11;
  bool use_jit = false;
  bool trace = false;
  bool trace_jit = false;
  bool big_endian_data = false;
  bool high_vectors = false;
  bool jazelle_trap = true;
  bool strict_unpredictable = false;
  bool serror_on_busfault = true;
  u32 max_block_insns = 48;
  bool jit_fast_mem = true;
  u32 jit_cache_mb = 64;
  bool cycle_model = true;
};

/* The counters are the core's `CpuStats` (they are what the C API reports),
 * spelled `Stats` here because that is what the rest of this module calls
 * them.  The exception ids are the ISA's (armv5::EXC_*), which is why the
 * array is generic-sized (16) and only EXC_COUNT entries are ever used. */
using Stats = CpuStats;

/* ================================================================== Cpu === = */

struct Jit;

/* The architectural CP15 register file (plain data; behaviour in cp15.cpp). */
struct Cp15Shadow {
  u32 c[CPCOUNT];
};

struct Cpu : public armv5::IsaState {
  /* Everything a Cpu owns is released here: the TLB table, and - through
   * `jit_enable(false)` - the JIT's code cache and its arena.  `init` may run
   * more than once on the same object (a machine is rebuilt after a reset), so
   * the TLB table is replaced rather than appended to. */
  ~Cpu();

  /* ------------- core state (the architectural half is the base) --------
   * The fields the JIT dispatcher touches per block entry: the architectural
   * ones (regs, cpsr, vflag, ...) are inherited from IsaState; these are the
   * core's own. */
  u32 halted;       /* low-power: waiting for an interrupt                  */
  u32 exit_req;     /* stop the run loop ASAP                               */
  u32 jit_flags;    /* JITF_* maintained by generated code                  */
  u32 trans_gen;    /* bumped on any translation-visible change;            */
                    /* every block stores the gen it was built with         */
  Bus* bus;         /* -> &bus_impl                                         */
  Jit* jit;         /* null when the JIT is off                             */
  u8* ram;          /* flat RAM base (identity fast path)                   */
  u32 ram_gbase;
  u32 ram_mask;     /* power-of-two size - 1 (0 => no fast path)            */
  u32 ram_fast;     /* MMU off and RAM window covers the address            */
  u32 smc_watch;    /* stores must be checked against code pages           */
  TlbEntry* tlb_d;
  TlbEntry* tlb_i;
  u32 tlb_mask;
  u32 mmu_on;
  u32 align_chk;    /* (SCTLR.A | CPSR.A) enabled alignment checks          */
  u32 be_data;      /* swap data accesses (BE8)                            */
  u32 domain;       /* CP15 c3 cache                                        */
  u32 ttb;          /* CP15 c2 cache (bits [31:14])                        */
  u32 cp15c1;       /* CP15 c1 cache                                        */
  u32 jit_entry;    /* host addr of the block for npc (dispatcher)          */
  u32 jit_slow;     /* reason the dispatcher was re-entered                 */

  Cp15Shadow cp15;
  u32 irq_line, fiq_line;
  TlbEntry* fetch_e;    /* validated translation of `cur` for this cycle      */
  u32 fetch_e_vpn;      /* page it belongs to (invalidation guard)            */

  /* The ISA engine's view of this core: the environment contract the engine
   * (arch/arm/isa/armv5/interp*.cpp) calls through.  Wired once, in the
   * constructor, to this object. */
  armv5::IsaEnv env;

  CpuConfig cfg;
  Stats stats;
  Bus bus_impl;       /* the address space; `bus` points here (the JIT keeps a
                         pointer slot so a device model can be swapped in)   */
  TlbEntry* tlb_store = nullptr;  /* owned; see ~Cpu */
  u32 trace_en;
  u32 running;

  /* linux-user glue */
  u32 syscallno, sys_ret, exit_code;
  u64 syscall_count;

  /* ------------------------------------------------------- methods --- */
  Cpu() { memset((void*)this, 0, sizeof(*this)); init_env(); }
  /* Wire `env` to this object (the IsaEnv callbacks are static functions
   * taking the Cpu* as their context). */
  void init_env();

  /* lifecycle */
  bool init(const CpuConfig& c);          /* owns its own bus (standalone use) */
  /* Bind to a bus the caller owns - what the machine core does.  The bus must
   * outlive the CPU. */
  bool attach(Bus& bus, const CpuConfig& c);
  void reset();
  /* Called by the bus when a store lands in a page that holds translated code:
   * the one thing this module wants from the address space. */
  static void on_bus_store(void* user, u32 addr, u32 size);
  /* Bounds are deltas; 0 means "no bound" on that axis.  See CpuCore::run_until
   * in the core: the same contract, and the only run loop this module has. */
  void run_until(u64 max_insns, u64 max_cycles);
  void step_one();                         /* exactly one instruction        */
  void request_stop() { exit_req = 1; }
  bool stop_requested() const { return exit_req != 0; }
  bool jit_enable(bool on);
  bool jit_enabled() const { return jit != nullptr; }

  /* ------------------------- ISA engine entry points -------------------
   * Thin forwarders into the ISA layer and the AArch32 execution state, so
   * the JIT's out-of-line helpers and the tests name exactly one
   * implementation of each rule.  The architectural accessors (set_thumb,
   * condition_true, set_nz, set_cpsr_flags, set_q, set_reg, switch_mode,
   * flush_bank, load_bank, bank_for_mode, mode, reg, pc_pipeline, carry,
   * overflow, qflag, insn_size, pc_bias, ...) are inherited from IsaState. */
  void exec_arm(u32 insn_word) { armv5::exec_arm(*this, insn_word, env); }
  void exec_arm_dec(const armv5::ArmDec& d) { armv5::exec_arm_dec(*this, d, env); }
  void exec_thumb(u32 hw) { armv5::exec_thumb(*this, hw, env); }
  void fetch_and_execute() { aarch32::fetch_and_execute(*this, env); }
  armv5::Shifted arm_op2(const armv5::ArmDec& d) const {
    return armv5::arm_op2(*this, d);
  }
  u32 arm_ldst_offset(const armv5::ArmDec& d) const {
    return armv5::arm_ldst_offset(*this, d);
  }
  void arm_block_addrs(const armv5::ArmDec& d, u32 n, u32* start, u32* wb) const {
    armv5::arm_block_addrs(*this, d, n, start, wb);
  }
  void restore_cpsr_from_spsr() { armv5::restore_cpsr_from_spsr(*this, env); }
  void load_pc(u32 value, bool from_load = false) {
    armv5::load_pc(*this, value, from_load);
  }
  void apply_msr(u32 mask, u32 val, bool spsr) {
    armv5::apply_msr(*this, mask, val, spsr, env);
  }

  /* Instruction fetch through the validated-translation cache (the slow path
   * is the MMU + bus; the fast path is the TLB entry's host pointer). */
  ALWAYS_INLINE u32 fetch16(GAddr a);
  ALWAYS_INLINE u32 fetch32(GAddr a);

  /* exceptions (core side: FSR/FAR into CP15, accounting, JIT notification;
   * the architectural half is armv5::enter_exception) */
  void take_exception(u32 e);          /* raised by the instruction at `cur` */
  void take_interrupt(u32 e);          /* taken in front of `npc`            */
  void enter_exception(u32 e, u32 vector, u32 mode, u32 lr);
  void raise_undef(u32 i) { armv5::raise_undef(*this, i); }
  void raise_swi(u32 imm) { armv5::raise_swi(*this, imm); }
  void raise_dabort(u32 fs, u32 va) { armv5::raise_dabort(*this, env, fs, va); }
  void raise_pabort(u32 fs) { armv5::raise_pabort(*this, env, fs); }
  bool check_interrupts();
  bool prepare_fetch();                    /* prefetch abort check for `cur`  */

  /* memory (MMU-aware; also used by JIT helper calls) */
  u32 read8(u32 a) { return mem_read(a, 1, armv5::MF_SZ8); }
  u32 read16(u32 a) { return mem_read(a, 2, armv5::MF_SZ16); }
  u32 read32(u32 a) { return mem_read(a, 4, armv5::MF_SZ32); }
  u32 read32_fetch(u32 a) { return mem_read(a, 4, armv5::MF_SZ32 | armv5::MF_FETCH); }
  void write8(u32 a, u32 val) { mem_write(a, val, 1, armv5::MF_WRITE | armv5::MF_SZ8); }
  void write16(u32 a, u32 val) { mem_write(a, val, 2, armv5::MF_WRITE | armv5::MF_SZ16); }
  void write32(u32 a, u32 val) { mem_write(a, val, 4, armv5::MF_WRITE | armv5::MF_SZ32); }

  ALWAYS_INLINE u32 mem_read(u32 addr, u32 size, u32 flags);
  ALWAYS_INLINE void mem_write(u32 addr, u32 val, u32 size, u32 flags);
  u32 mem_read_slow(u32 addr, u32 size, u32 flags);
  void mem_write_slow(u32 addr, u32 val, u32 size, u32 flags);

  /* physical (bus-level, no MMU) -- loaders, debuggers, tests */
  u32 phys_read(u32 a, u32 size);
  void phys_write(u32 a, u32 val, u32 size);
  bool load_image(u32 a, const void* p, size_t n);

  /* MMU / CP15 */
  bool mmu_translate(u32 va, u32 flags, TlbEntry* out);
  TlbEntry* tlb_lookup(u32 va, u32 flags);
  void tlb_flush_all();
  void tlb_flush_page(u32 mva);
  /* `val` is the register value to write (the ISA engine reads the source
   * register and passes it through IsaEnv::coproc_write). */
  void cp15_write(u32 crn, u32 opc1, u32 val, u32 crm, u32 opc2);
  u32 cp15_read(u32 crn, u32 opc1, u32 crm, u32 opc2);
  void sync_cp15_caches();
  void on_translation_env_changed(const char* why);
  void invalidate_code(u32 addr, u32 size);

  /* The ISA engine's coprocessor view of CP15 (IsaEnv::coproc): which
   * cp/opc1/alias pairs exist, and the transfers themselves. */
  bool coproc_present(u32 cp, u32 opc1, bool second) const;
  void coproc_write(u32 cp, u32 crn, u32 opc1, u32 crm, u32 opc2, u32 val);
  u32 coproc_read(u32 cp, u32 crn, u32 opc1, u32 crm, u32 opc2);

  /* JIT helper ABI (referenced by name from generated code) */
  static void h_raise(Cpu* c, u32 e);
  static u32 h_read32(Cpu* c, u32 addr);
  static u32 h_read16(Cpu* c, u32 addr);
  static u32 h_read8(Cpu* c, u32 addr);
  static void h_write32(Cpu* c, u32 addr, u32 val);
  static void h_write16(Cpu* c, u32 addr, u32 val);
  static void h_write8(Cpu* c, u32 addr, u32 val);
  static void h_msr_cpsr(Cpu* c, u32 mask, u32 val);
  static void h_msr_spsr(Cpu* c, u32 mask, u32 val);
  static void h_exec_arm(Cpu* c, u32 insn);   /* opaque: run the interpreter */
  static void h_exec_thumb(Cpu* c, u32 insn);
  static void h_branch_exchange(Cpu* c, u32 target);
  static void h_check_gen(Cpu* c, u32 gen);

  void trace_instruction();
  void trace(const char* what);
};

/* ------------------------------------------------------- inline hot bits */

ALWAYS_INLINE u32 Cpu::fetch16(GAddr a) {
  if (BRAIN_LIKELY(fetch_e != nullptr && fetch_e->host)) {
    const u8* p = fetch_e->host + (a & 0xFFF);
    return (u32)p[0] | ((u32)p[1] << 8);
  }
  return mem_read(a, 2, armv5::MF_SZ16 | armv5::MF_FETCH);
}
ALWAYS_INLINE u32 Cpu::fetch32(GAddr a) {
  if (BRAIN_LIKELY(fetch_e != nullptr && fetch_e->host)) {
    u32 out;
    memcpy(&out, fetch_e->host + (a & 0xFFF), 4);
    return out;
  }
  return mem_read(a, 4, armv5::MF_SZ32 | armv5::MF_FETCH);
}

ALWAYS_INLINE u32 Cpu::mem_read(u32 addr, u32 size, u32 flags) {
  if (BRAIN_UNLIKELY(align_chk && !(flags & (armv5::MF_UNAL | armv5::MF_FETCH)) &&
                     (addr & (size - 1))))
    return mem_read_slow(addr, size, flags);
  /* Identity fast path: MMU off and the address falls inside the RAM window. */
  if (BRAIN_LIKELY(ram_fast && ((addr - ram_gbase) & ~ram_mask) == 0)) {
    const u8* p = ram + (addr & ram_mask);
    u32 res;
    if (size == 4) {
      memcpy(&res, p, 4);
    } else if (size == 2) {
      res = (u32)p[0] | ((u32)p[1] << 8);
    } else {
      res = p[0];
    }
    if (BRAIN_UNLIKELY(be_data && size == 2)) {
      /* BE8 inverts the two lanes of a halfword access.  A 32-bit access is
       * not swapped here: the byte order of words comes from the image
       * loading (CpuConfig::big_endian_code), which keeps a load and the
       * matching store mutual inverses without a second swap. */
      res = be32_swap16(res);
    }
    return res;
  }
  return mem_read_slow(addr, size, flags);
}

ALWAYS_INLINE void Cpu::mem_write(u32 addr, u32 val, u32 size, u32 flags) {
  if (BRAIN_UNLIKELY(align_chk && !(flags & (armv5::MF_UNAL | armv5::MF_FETCH)) &&
                     (addr & (size - 1)))) {
    mem_write_slow(addr, val, size, flags);
    return;
  }
  if (BRAIN_LIKELY(ram_fast && ((addr - ram_gbase) & ~ram_mask) == 0)) {
    u8* p = ram + (addr & ram_mask);
    if (BRAIN_UNLIKELY(be_data && size == 2)) {
      val = be32_swap16(val);
    }
    if (size == 4) {
      memcpy(p, &val, 4);
    } else if (size == 2) {
      p[0] = (u8)val; p[1] = (u8)(val >> 8);
    } else {
      p[0] = (u8)val;
    }
    if (BRAIN_UNLIKELY(smc_watch)) bus->note_store(addr, size);
    return;
  }
  mem_write_slow(addr, val, size, flags);
}

}  // namespace brain::arm926

#endif  /* BRAINEMU_CORE_STATE_H */
