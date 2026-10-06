/* brain-emu -- ARM926EJ-S (ARMv5TEJ) core: state layout + hot paths.
 *
 * This header is the ABI between the interpreter, the JIT dispatcher, the IR
 * and both code generators.  Generated host code touches guest state only
 * through offsetof(Cpu, field) so the layout can evolve as long as the
 * invariants below hold:
 *
 *   - the "hot" block at the top of the struct is POD and stays within a
 *     couple of cache lines;
 *   - Rn of the *current* mode is always `cpu->regs[n]` (banked registers are
 *     swapped in/out on mode change, which keeps every data-path access a
 *     single base+disp load);
 *   - N/Z/C may live in the host flag register while `jit_flags &
 *     JITF_HOST_NZC` is set; V is *always* materialised in `cpu->vflag`.
 *     Rationale: ARM leaves V untouched for shifts/MOV/MUL while x86/AArch64
 *     logical and shift ops destroy it -- mirroring V in memory removes every
 *     "preserve OF" dance from the code generators.
 *
 * References: ARM DDI 0100I (ARM ARM v5), ARM DDI 0198E (ARM926EJ-S TRM).
 */
#ifndef BRAINEMU_CORE_STATE_H
#define BRAINEMU_CORE_STATE_H

#include <cstddef>
#include <cstring>

#include "brainemu/bus.h"
#include "brainemu/types.h"

namespace brain {

struct ArmDec;

/* Barrel-shifter result: value plus the carry the shift produced. */
enum : u32 { SH_LSL = 0, SH_LSR = 1, SH_ASR = 2, SH_ROR = 3 };
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

/* Fault status: the ARM926EJ-S (ARMv4/v5 style) encoding of TRM table 3-9.
 * FSR[7:4] = domain, FSR[11] = fault came from a second level (page) table,
 * FSR[10] = write access, FSR[9] = section translation fault is *not* used
 * here; bit 8 = external abort on linefill (unused). */
enum : u32 {
  FS926_ALIGN         = 0x01, /* b00x1, alignment                             */
  FS926_EXT_LINEFILL  = 0x08, /* b10x0, external abort on linefill            */
  FS926_TRANS_SECTION = 0x05, /* b0101, translation fault, first level         */
  FS926_TRANS_PAGE    = 0x07, /* b0111, translation fault, second level        */
  FS926_DOMAIN_SECT   = 0x09, /* b1001, domain fault, first level             */
  FS926_DOMAIN_PAGE   = 0x0B, /* b1011, domain fault, second level            */
  FS926_PERM_SECT     = 0x0D, /* b1101, permission fault, first level          */
  FS926_PERM_PAGE     = 0x0F, /* b1111, permission fault, second level         */
  FS926_EXT_TRANS_L1  = 0x0C, /* b1100, external abort on first level table walk*/
  FS926_EXT_TRANS_L2  = 0x0E, /* b1110, external abort on second level walk     */
};
enum : u32 {
  /* TRM table 2-15: the FSR is only [3:0] fault type and [7:4] domain - bit
   * [8] "always reads as zero, writes ignored" and [31:9] UNP/SBZ, so there
   * is no write bit and no level bit here (the level is part of the 4-bit
   * fault type, table 3-9).  Nothing else may be OR-ed into what the guest
   * reads back. */
  FSR_DOMAIN_SHIFT = 4,
};

/* BE32 (CP15 c1[B]) swaps the byte order inside a word: a halfword access
 * therefore sees its two bytes in the opposite order.  Keep the result to 16
 * bits - the shift-then-or form would otherwise leak the upper half. */
ALWAYS_INLINE u32 be32_swap16(u32 v) { return ((v >> 8) | (v << 8)) & 0xFFFFu; }

/* Memory-access flags.  The fault kind is derived from these: MF_FETCH raises
 * a prefetch abort, everything else a data abort.  MF_UNAL marks an access
 * that must never take an alignment fault: byte accesses and instruction
 * fetches, the two exemptions of TRM 3.5.1.  (Word and halfword accesses keep
 * their addresses even when CP15 c1[A] is clear, so they always reach the bus
 * with the effective address and no rotate is modelled - see
 * docs/ISA-COVERAGE.md.) */
enum : u32 {
  MF_WRITE = 1u << 0,
  MF_FETCH = 1u << 1,
  MF_SWP   = 1u << 2,   /* read-modify-write: needs read *and* write perms  */
  MF_UNAL  = 1u << 3,
  MF_SZ_M  = 3u << 4,
  MF_SZ8   = 0u << 4,
  MF_SZ16  = 1u << 4,
  MF_SZ32  = 2u << 4,
};

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
  bool jit_chain = true;
  bool jit_fast_mem = true;
  u32 jit_cache_mb = 64;
  bool cycle_model = true;
};

struct Stats {
  u64 insns_interp = 0, insns_jit = 0;
  u64 blocks = 0, links = 0, flushes = 0, smc_flushes = 0;
  u64 tlb_hits = 0, tlb_miss = 0, walks = 0, page_faults = 0;
  u64 exc[EXC_COUNT] = {};
  u64 cycles_interp = 0, cycles_jit = 0;
  u64 code_bytes = 0;
  u64 branch_taken = 0, branch_not = 0;
  u64 helper_calls = 0;
};

/* ================================================================== Cpu === = */

struct Jit;

/* The architectural CP15 register file (plain data; behaviour in cp15.cpp). */
struct Cp15Shadow {
  u32 c[CPCOUNT];
};

struct Cpu {
  /* ------------------------- hot: shared with generated code ---------- */
  u32 regs[16];     /* +0x00  R15 = pipeline value (cur+8 ARM / cur+4 T)   */
  u32 npc;          /* +0x40  address of the next instruction to execute    */
  u32 cur;          /* +0x44  address of the instruction being executed      */
  u32 insn;         /* +0x48  instruction word (SWI/undef reporting)         */
  u32 cpsr;         /* +0x4C                                              */
  u32 vflag;        /* +0x50  V flag (always materialised)                 */
  u32 thumb;        /* +0x54  1 while in Thumb state                       */
  u32 halted;       /* +0x58  low-power: waiting for an interrupt           */
  u32 exit_req;     /* +0x5C  stop the run loop ASAP                        */
  u64 cycles;       /* +0x60                                              */
  u64 insns;        /* +0x68                                              */
  u32 jit_flags;    /* +0x70  JITF_* maintained by generated code           */
  u32 trans_gen;    /* +0x74  bumped on any translation-visible change;     */
                    /*        every block stores the gen it was built with   */
  Bus* bus;         /* +0x78  -> &bus_obj (kept so generated code has one
                     *        indirection slot if the bus is ever relocated)  */
  Jit* jit;         /* +0x80  null when the JIT is off                      */
  u8* ram;          /* +0x88  flat RAM base (identity fast path)            */
  u32 ram_gbase;    /* +0x90                                              */
  u32 ram_mask;     /* +0x94  power-of-two size - 1 (0 => no fast path)     */
  u32 ram_fast;     /* +0x98  MMU off and RAM window covers the address     */
  u32 exc;          /* +0x9C  exception currently being taken               */
  u32 exc_fsr;      /* +0xA0  value destined for DFSR/RFSR                  */
  u32 exc_far;      /* +0xA4  value destined for FAR                       */
  u32 smc_watch;    /* +0xA8  stores must be checked against code pages      */
  TlbEntry* tlb_d;  /* +0xB0                                              */
  TlbEntry* tlb_i;  /* +0xB8                                              */
  u32 tlb_mask;     /* +0xC0                                              */
  u32 mmu_on;       /* +0xC4                                              */
  u32 align_chk;    /* +0xC8  (SCTLR.A | CPSR.A) enabled alignment checks   */
  u32 be_data;      /* +0xCC  swap data accesses (BE8)                     */
  u32 priv;         /* +0xD0  1 when mode != USR (for AP checks)           */
  u32 domain;       /* +0xD4  CP15 c3 cache                                 */
  u32 ttb;          /* +0xD8  CP15 c2 cache (bits [31:14])                 */
  u32 cp15c1;       /* +0xDC  CP15 c1 cache                                 */
  u32 jit_entry;    /* +0xE0  host addr of the block for npc (dispatcher)   */
  u32 jit_slow;     /* +0xE4  reason the dispatcher was re-entered          */
  u32 pad_hot[2];   /* +0xE8                                              */
  /* --------------------------- end of hot region --------------------- */

  struct Bank {
    u32 r[16];
    u32 spsr;
  };
  Bank bank[BANK_COUNT];
  u32 bank_idx;
  Cp15Shadow cp15;      /* see below: plain c[] array + derived bits */
  u32 irq_line, fiq_line;
  u32 exectrap;         /* pending exception that aborted an instruction */
  TlbEntry* fetch_e;    /* validated translation of `cur` for this cycle   */
  u32 fetch_e_vpn;      /* page it belongs to (invalidation guard)         */

  CpuConfig cfg;
  Stats stats;
  Bus bus_impl;       /* the address space; `bus` points here (the JIT keeps a
                         pointer slot so a device model can be swapped in)     */
  TlbEntry* tlb_store;
  u32 stop_after;
  u32 trace_en;
  u32 running;

  /* linux-user glue */
  u32 syscallno, sys_ret, exit_code;
  u64 syscall_count;

  /* ------------------------------------------------------- methods --- */
  Cpu() { memset((void*)this, 0, sizeof(*this)); }

  u32 reg(u32 n) const { return regs[n]; }
  u32 pc_pipeline() const { return regs[15]; }
  u32 mode() const { return cpsr & CPSR_MODE_M; }
  bool thumb_mode() const { return thumb != 0; }
  u32 cur_insn_size() const { return thumb ? 2 : 4; }
  u32 carry() const { return (cpsr >> 29) & 1; }
  u32 overflow() const { return (cpsr >> 28) & 1; }
  u32 qflag() const { return (cpsr >> 27) & 1; }

  void set_reg(u32 n, u32 val);            /* handles PC writes + banking   */
  void set_cpsr_flags(u32 n, u32 z, u32 c, u32 vv);
  void set_nz(u32 res);
  void set_q(bool on) { cpsr = (cpsr & ~CPSR_Q) | (on ? u32(CPSR_Q) : 0u); }
  bool condition_true(u32 cond) const;

  /* lifecycle */
  bool init(const CpuConfig& c);
  void reset();
  void run(u64 max_cycles);
  void run_insns(u64 n);
  void step_one();                         /* exactly one instruction        */
  void request_stop() { exit_req = 1; }
  bool jit_enable(bool on);
  bool jit_enabled() const { return jit != nullptr; }

  /* interpreter entry points (also used as the JIT's out-of-line helper) */
  void exec_arm(u32 insn);
  void exec_arm_dec(const ArmDec& d);    /* shared by both states            */
  void exec_thumb(u32 hw);                 /* Thumb state, one halfword        */
  void fetch_and_execute();                /* one instruction, both states      */

  /* Pipeline rule, maintained by Cpu::fetch_and_execute and mirrored by the
   * JIT at each block entry: `cur` is the address of the instruction being
   * executed, `npc` the address of the next one, and regs[15] the value the
   * executing instruction reads when it names R15 (cur+8 in ARM state, cur+4
   * in Thumb state -- A2.4.3). */
  u32 insn_size() const;                   /* 4 in ARM state, 2 in Thumb      */
  u32 pc_bias() const;                     /* 8 in ARM state, 4 in Thumb      */
  ALWAYS_INLINE u32 fetch16(GAddr a);
  ALWAYS_INLINE u32 fetch32(GAddr a);

  /* exceptions */
  void take_exception(u32 e);
  u32 exception_lr_offset(u32 e) const;
  void raise_undef(u32 insn);
  void raise_swi(u32 imm);
  void raise_dabort(u32 fs, u32 va);
  void raise_pabort(u32 fs);
  bool check_interrupts();
  bool prepare_fetch();                    /* prefetch abort check for `cur`  */

  /* mode/bank switching */
  void switch_mode(u32 new_mode);
  void flush_bank();
  void load_bank(u32 idx);
  u32 bank_for_mode(u32 mode) const;
  void enter_exception(u32 e, u32 vector, u32 mode, u32 lr_adjust);

  /* memory (MMU-aware; also used by JIT helper calls) */
  u32 read8(u32 a) { return mem_read(a, 1, MF_SZ8); }
  u32 read16(u32 a) { return mem_read(a, 2, MF_SZ16); }
  u32 read32(u32 a) { return mem_read(a, 4, MF_SZ32); }
  u32 read32_fetch(u32 a) { return mem_read(a, 4, MF_SZ32 | MF_FETCH); }
  void write8(u32 a, u32 val) { mem_write(a, val, 1, MF_WRITE | MF_SZ8); }
  void write16(u32 a, u32 val) { mem_write(a, val, 2, MF_WRITE | MF_SZ16); }
  void write32(u32 a, u32 val) { mem_write(a, val, 4, MF_WRITE | MF_SZ32); }

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
  void cp15_write(u32 crn, u32 opc1, u32 rd, u32 crm, u32 opc2);
  u32 cp15_read(u32 crn, u32 opc1, u32 crm, u32 opc2);
  void sync_cp15_caches();
  void on_translation_env_changed(const char* why);
  void invalidate_code(u32 addr, u32 size);

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

  /* interpreter entry points shared with the JIT (declared here so both
   * engines use exactly one implementation of each rule). */
  ALWAYS_INLINE Shifted arm_op2(const ArmDec& d) const;
  u32 arm_ldst_offset(const ArmDec& d) const;
  void arm_block_addrs(const ArmDec& d, u32 n, u32* start, u32* wb) const;
  void restore_cpsr_from_spsr();
  void load_pc(u32 value);
  void apply_msr(u32 mask, u32 val, bool spsr);
  static bool valid_mode(u32 m);
  void trace_instruction();

  void trace(const char* what);

};


/* ------------------------------------------------------- inline hot bits */

ALWAYS_INLINE bool Cpu::condition_true(u32 cond) const {
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

ALWAYS_INLINE void Cpu::set_nz(u32 res) {
  cpsr = (cpsr & ~(CPSR_N | CPSR_Z)) | (res & CPSR_N) | (res == 0 ? u32(CPSR_Z) : 0u);
}

ALWAYS_INLINE void Cpu::set_cpsr_flags(u32 n, u32 z, u32 c, u32 vv) {
  cpsr = (cpsr & ~CPSR_FLAGS) | (n ? u32(CPSR_N) : 0u) | (z ? u32(CPSR_Z) : 0u) |
         (c ? u32(CPSR_C) : 0u) | (vv ? u32(CPSR_V) : 0u);
  vflag = vv ? 1 : 0;
}

ALWAYS_INLINE u32 Cpu::mem_read(u32 addr, u32 size, u32 flags) {
  if (BRAIN_UNLIKELY(align_chk && !(flags & (MF_UNAL | MF_FETCH)) &&
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
  if (BRAIN_UNLIKELY(align_chk && !(flags & (MF_UNAL | MF_FETCH)) &&
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
    if (BRAIN_UNLIKELY(smc_watch)) bus->note_store(addr);
    return;
  }
  mem_write_slow(addr, val, size, flags);
}

}  // namespace brain

#endif  /* BRAINEMU_CORE_STATE_H */
