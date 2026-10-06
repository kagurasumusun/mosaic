/* brain-emu -- MMU, CP15 and FCSE tests for the ARM926EJ-S core.
 * SPDX-file-note: see LICENSE.
 *
 * Everything here goes through the public C API, because that is the only
 * honest way to test a memory map: page tables are poked into RAM as
 * guest-physical data, CP15 is configured by hand-encoded MCR/MRC
 * instructions, and faults are inspected by a *guest* abort handler that reads
 * the DFSR/IFSR and FAR back out of CP15.  Nothing reaches into Cpu, so a test
 * can only pass by agreeing with the manual, not with the implementation.
 *
 * Bit positions and semantics come from ARM DDI 0198E: descriptors from tables
 * 3-4 (section), 3-5 (coarse), 3-6 (fine) and 3-7/3-8 (second level), the
 * subpage permission rule from 3.5.4, permissions from tables 3-11 (domain
 * access control) and 3-12 (AP plus the legacy ARMv3 S/R encodings), the fault
 * checking sequence and priority from 3.5 and table 3-9, FSR/FAR from tables
 * 2-15/2-16 and sections 2.3.6/2.3.7, the FCSE from 2.3.13, and the CP15 c0/c1
 * behaviour from tables 2-5, 2-10 and 2-11.
 */
#include <brainemu/brainemu.h>

#include <cstdio>
#include <cstring>

namespace {

int g_checks = 0, g_failed = 0;

void check(bool ok, const char* what) {
  ++g_checks;
  if (!ok) {
    ++g_failed;
    std::printf("  FAILED: %s\n", what);
  }
}

#define CHECK(cond) check((cond), #cond)

/* ---------------------------------------------------------------- layout -- */

const uint32_t CODE = 0x8000u;       /* guest program, identity mapped below */
const uint32_t L1 = 0x10000u;        /* level-one table, 16 KiB aligned       */
const uint32_t L2 = 0x12000u;        /* coarse table: 256 entries             */
const uint32_t L2F = 0x13000u;       /* fine table: 1024 entries              */
const uint32_t RAM_SZ = 2u << 20;

/* Low vectors (ARMv5, CP15 c1[13] = 0) plus the handlers installed below them.
 */
const uint32_t V_PABORT = 0x0Cu, V_DABORT = 0x10u;
const uint32_t H_PABORT = 0x20u, H_DABORT = 0x30u;

const uint32_t SCTLR_M = 1u << 0;    /* MMU enable                            */
const uint32_t SCTLR_A = 1u << 1;    /* alignment fault checking              */
const uint32_t SCTLR_B = 1u << 7;    /* data endianness                       */
const uint32_t SCTLR_S = 1u << 8;    /* ARMv3-compatible segment permissions   */
const uint32_t CPSR_E = 1u << 9;
const uint32_t MODE_SVC = 0x13u, MODE_ABT = 0x17u;

/* Domain access control values, table 3-11. */
const uint32_t DOM_NO = 0, DOM_CLIENT = 1, DOM_RESERVED = 2, DOM_MANAGER = 3;

/* Fault types, tables 2-16 / 3-9. */
const uint32_t FS_ALIGN = 0x1, FS_TRANS_SECT = 0x5, FS_TRANS_PAGE = 0x7,
               FS_DOMAIN_SECT = 0x9, FS_DOMAIN_PAGE = 0xB, FS_PERM_SECT = 0xD,
               FS_PERM_PAGE = 0xF;

uint32_t dacc_of(uint32_t dom, uint32_t acc) { return acc << (dom * 2); }

/* ---------------------------------------------- descriptor builders ------ */

/* Section: base [31:20], AP [11:10], domain [8:5], [4] = 1, CB [3:2], tag 10. */
uint32_t sect(uint32_t pa, uint32_t dom, uint32_t ap, uint32_t cb = 3) {
  return (pa & 0xFFF00000u) | (ap << 10) | (dom << 5) | (1u << 4) | (cb << 2) |
         2u;
}
/* Coarse table: base [31:10], domain [8:5], [4] = 1, [3:2] = 0, tag 01. */
uint32_t l1_coarse(uint32_t pt, uint32_t dom) {
  return (pt & 0xFFFFFC00u) | (dom << 5) | (1u << 4) | 1u;
}
/* Fine table: base [31:12], domain [8:5], [4] = 1, [3:2] = 0, tag 11. */
uint32_t l1_fine(uint32_t pt, uint32_t dom) {
  return (pt & 0xFFFFF000u) | (dom << 5) | (1u << 4) | 3u;
}
/* Small page (4 KiB): base [31:12], one AP pair per 1 KiB subpage at [11:4];
 * ap0 covers the bottom KiB and ap3 the top (3.5.4). */
uint32_t l2_small(uint32_t pa, uint32_t ap0, uint32_t ap1 = 0, uint32_t ap2 = 0,
                  uint32_t ap3 = 0) {
  return (pa & 0xFFFFF000u) | (ap3 << 10) | (ap2 << 8) | (ap1 << 6) |
         (ap0 << 4) | (3u << 2) | 2u;
}
/* Large page (64 KiB): base [31:16], one AP pair per 16 KiB subpage. */
uint32_t l2_large(uint32_t pa, uint32_t ap0, uint32_t ap1 = 0, uint32_t ap2 = 0,
                  uint32_t ap3 = 0) {
  return (pa & 0xFFFF0000u) | (ap3 << 10) | (ap2 << 8) | (ap1 << 6) |
         (ap0 << 4) | (3u << 2) | 1u;
}
/* Tiny page (1 KiB): base [31:10], a single AP at [5:4], tag 11. */
uint32_t l2_tiny(uint32_t pa, uint32_t ap) {
  return (pa & 0xFFFFFC00u) | (ap << 4) | (3u << 2) | 3u;
}

uint32_t l1_index(uint32_t va) { return (va >> 20) & 0xFFFu; }
uint32_t l2_index_coarse(uint32_t va) { return (va >> 12) & 0xFFu; }
uint32_t l2_index_fine(uint32_t va) { return (va >> 10) & 0x3FFu; }

/* ------------------------------------------------------------- programs -- */

/* Hand-encoded; the instruction corpus owns semantics, these only have to
 * retire and leave the registers the test reads back. */
enum : uint32_t {
  kMcrTtbR1 = 0xEE021F10u,   /* mcr p15,0,r1,c2,c0,0 translation table base    */
  kMcrDaccR2 = 0xEE032F10u,  /* mcr p15,0,r2,c3,c0,0 domain access control     */
  kMcrPidR3 = 0xEE0D3F10u,   /* mcr p15,0,r3,c13,c0,0 FCSE process ID          */
  kMcrCtxR3 = 0xEE0D3F30u,   /* mcr p15,0,r3,c13,c0,1 context ID               */
  kMrcPidR1 = 0xEE1D1F10u,   /* mrc p15,0,r1,c13,c0,0 FCSE process ID         */
  kMrcCtxR2 = 0xEE1D2F30u,
  kMrcCtxR1 = 0xEE1D1F30u,   /* mrc p15,0,r1,c13,c0,1                          */
  kMcrCtrlR0 = 0xEE010F10u,  /* mcr p15,0,r0,c1,c0,0 control: the MMU comes on */
  kTlbiAllR0 = 0xEE080F17u,  /* mcr p15,0,r0,c8,c7,0  invalidate the whole TLB */
  kMrcCtrlR3 = 0xEE113F10u,  /* mrc p15,0,r3,c1,c0,0                           */
  kMrcIdR4 = 0xEE104F10u,    /* mrc p15,0,r4,c0,c0,0  ID code, table 2-5       */
  kMrcCtypeR5 = 0xEE105F30u, /* mrc p15,0,r5,c0,c0,1  cache type, table 2-10   */
  kMrcTcmR6 = 0xEE106F50u,   /* mrc p15,0,r6,c0,c0,2  TCM status              */
  kMrcDfsrR7 = 0xEE157F10u,  /* mrc p15,0,r7,c5,c0,0  data fault status        */
  kMrcIfsrR7 = 0xEE157F30u,  /* mrc p15,0,r7,c5,c0,1  instruction fault status */
  kMrcFarR6 = 0xEE166F10u,   /* mrc p15,0,r6,c6,c0,0  fault address            */
  kLdrR2r5 = 0xE5952000u,    /* ldr r2, [r5]                                   */
  kAddR2 = 0xE2822001u,      /* add r2, r2, #1                                 */
  kStrR2r5 = 0xE5852000u,    /* str r2, [r5]                                   */
  kLdrhR2r5 = 0xE1D520B0u,   /* ldrh r2, [r5]                                  */
  kLsr24R5 = 0xE1A05C24u,    /* mov r5, r4, lsr #24                            */
  kMovPcR0 = 0xE1A0F000u,    /* mov pc, r0                                     */
  kMovPcR5 = 0xE1A0F005u,    /* mov pc, r5                                     */
  kBself = 0xEAFFFFFEu,      /* b .  - parks the core inside the handler        */
};

/* Bring the MMU up the way a bootloader does, then copy one word through the
 * translation: read [r5], add one, write it back.  r0 = CP15 c1, r1 = the
 * level-one table base, r2 = c3, r3 = the FCSE PID (pre-shifted), r5 = the
 * access address. */
const uint32_t kProg[] = {kMcrTtbR1, kMcrDaccR2, kMcrPidR3, kMcrCtrlR0,
                          kLdrR2r5, kAddR2, kStrR2r5};
const size_t kProgN = sizeof kProg / sizeof kProg[0];
/* The same, with a TLB invalidation after the copy. */
const uint32_t kProgTlbi[] = {kMcrTtbR1, kMcrDaccR2, kMcrPidR3, kMcrCtrlR0,
                              kLdrR2r5, kAddR2, kStrR2r5, kTlbiAllR0};
const size_t kProgTlbiN = sizeof kProgTlbi / sizeof kProgTlbi[0];
/* Control write + one halfword load, for the endianness test. */
const uint32_t kProgBe[] = {kMcrCtrlR0, kLdrhR2r5};
const size_t kProgBeN = sizeof kProgBe / sizeof kProgBe[0];

/* ------------------------------------------------------------------ emu -- */

struct Emu {
  brain_emu_t* e = nullptr;

  explicit Emu(uint32_t ram_sz = RAM_SZ) {
    brain_config_t cfg;
    std::memset(&cfg, 0, sizeof cfg);
    cfg.ram_size = ram_sz;
    e = brain_emu_create(&cfg);
    CHECK(e != nullptr);
    if (!e) return;
    /* Abort handlers: pull the fault status and the FAR into r7/r6 and park.
     * Neither r6 nor r7 is banked in Abort mode, so the host can read them
     * back after the run - that is how this file inspects FSR and FAR. */
    brain_poke32(e, V_PABORT, 0xEA000003u);          /* b   H_PABORT         */
    brain_poke32(e, V_DABORT, 0xEA000006u);           /* b   H_DABORT          */
    brain_poke32(e, H_PABORT, kMrcIfsrR7);
    brain_poke32(e, H_PABORT + 4, kMrcFarR6);
    brain_poke32(e, H_PABORT + 8, kBself);
    brain_poke32(e, H_DABORT, kMrcDfsrR7);
    brain_poke32(e, H_DABORT + 4, kMrcFarR6);
    brain_poke32(e, H_DABORT + 8, kBself);
  }
  ~Emu() {
    if (e) brain_emu_destroy(e);
  }
  Emu(const Emu&) = delete;
  Emu& operator=(const Emu&) = delete;

  /* RAM starts at guest physical address 0, so a host poke is exactly what the
   * table walk will read back. */
  void put32(uint32_t pa, uint32_t v) { brain_poke32(e, pa, v); }
  uint32_t get32(uint32_t pa) { return brain_peek32(e, pa); }
  void put16(uint32_t pa, uint32_t v) { brain_poke16(e, pa, v); }
  void map_l1(uint32_t va, uint32_t desc) { put32(L1 + l1_index(va) * 4u, desc); }
  void map_l2(uint32_t pt, uint32_t idx, uint32_t desc) {
    put32(pt + idx * 4u, desc);
  }

  void setr(int r, uint32_t v) { CHECK(brain_set_reg(e, r, v) == 0); }
  uint32_t getr(int r) { return brain_get_reg(e, r); }
  /* `brain_pending_exception` only reports an exception the core has *not*
   * finished taking, so it reads back 0 once the vector has been entered: the
   * observable facts are the mode, the counters, and what the handler found in
   * the FSR/FAR. */
  int exc() const { return brain_pending_exception(e); }
  uint32_t mode() { return getr(16) & 0x1Fu; }
  bool in_abt() { return mode() == MODE_ABT; }
  uint32_t nfault(int kind) const {
    brain_stats_t st;
    brain_get_stats(e, &st);
    return (uint32_t)st.exc[kind];
  }
  uint32_t cpsr() { return brain_get_reg(e, BRAIN_REG_CPSR); }
  uint32_t fsr() { return getr(7) & 0xFu; }
  uint32_t fsr_dom() { return (getr(7) >> 4) & 0xFu; }
  uint32_t far() { return getr(6); }
  uint64_t go(uint64_t n) { return brain_run_insns(e, n); }

  void load(const uint32_t* prog, size_t n) {
    CHECK(brain_load_raw(e, prog, n * 4u, CODE, 0) == 0);
  }

  /* One independent phase: reset first (CP15 back to its reset values, so the
   * prologue always starts with the MMU off and the code reachable), then run
   * `run` instructions.  A fault is allowed to reach the vector and the
   * handler, hence the extra budget the callers pass. */
  void phase(const uint32_t* prog, size_t n, uint32_t c1, uint32_t dacc,
             uint32_t pid, uint32_t va, uint64_t run, uint32_t cpsr = ~0u) {
    brain_reset(e);
    if (cpsr != ~0u) setr(BRAIN_REG_CPSR, cpsr);
    load(prog, n);
    setr(0, c1);
    setr(1, L1);
    setr(2, dacc);
    setr(3, pid);
    setr(5, va);
    go(run);
  }
  /* Same, for the cases that must not fault: the whole program has to retire. */
  void clean(const uint32_t* prog, size_t n, uint32_t c1, uint32_t dacc,
             uint32_t pid, uint32_t va) {
    phase(prog, n, c1, dacc, pid, va, n);
    CHECK(exc() == BRAIN_EXC_NONE);
    CHECK(mode() == MODE_SVC);   /* nothing vectored: still in the caller's mode */
  }
  /* Re-enter the program at one of its own instructions, keeping CP15 *and*
   * the TLB: this is how stale cached translations get measured. */
  void again(size_t first, uint64_t n, uint32_t va) {
    setr(5, va);
    CHECK(brain_set_reg(e, 15, CODE + (uint32_t)(first * 4u)) == 0);
    go(n);
  }
};

/* A device that must be reached with PA = MVA = VA when the MMU is off. */
struct Dev {
  uint32_t value;
  int reads, writes;
} g_dev;

uint32_t dev_read(void*, uint32_t, uint32_t) {
  g_dev.reads++;
  return g_dev.value;
}
void dev_write(void*, uint32_t, uint32_t value, uint32_t) {
  g_dev.writes++;
  g_dev.value = value;
}

/* =============================== the tests ============================= */

/* Sections, small pages with per-KiB permissions, tiny pages through a fine
 * table, and the faults an unmapped or forbidden access reports. */
void test_translation() {
  std::printf("[translation]\n");
  Emu m;
  if (!m.e) return;

  const uint32_t dacc = dacc_of(0, DOM_MANAGER) | dacc_of(1, DOM_CLIENT);
  m.map_l1(0u, sect(0u, 0, 1));                    /* code, tables, vectors   */
  m.map_l1(0x00F00000u, sect(0x00100000u, 1, 1));   /* 1 MiB section           */
  m.map_l1(0x00200000u, l1_coarse(L2, 1));           /* small pages             */
  m.map_l2(L2, l2_index_coarse(0x00200000u),
           l2_small(0x00004000u, 1, 0, 0, 0));       /* only KiB 0 is usable    */
  m.map_l1(0x00400000u, l1_fine(L2F, 1));            /* tiny pages              */
  m.map_l2(L2F, l2_index_fine(0x00400000u), l2_tiny(0x00014000u, 1));
  m.put32(0x00100040u, 0x11223344u);
  m.put32(0x00004040u, 0xCAFEF00Du);
  m.put32(0x00014140u, 0x0BADF00Du);

  /* A section moves the whole 1 MiB block; only the word touched changes. */
  m.clean(kProg, kProgN, SCTLR_M, dacc, 0, 0x00F00040u);
  CHECK(m.getr(2) == 0x11223345u);
  CHECK(m.get32(0x00100040u) == 0x11223345u);
  CHECK(m.get32(0x0010003Cu) == 0u);               /* untouched neighbour     */
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x000FFFF0u, kProgN);
  CHECK(m.mode() == MODE_SVC);               /* same section, far end    */
  CHECK(m.get32(0x000FFFF0u) == 1u);

  /* Small page: the AP pair is selected by the 1 KiB subpage. */
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00200040u, kProgN);
  CHECK(m.mode() == MODE_SVC);
  CHECK(m.getr(2) == 0xCAFEF00Eu);
  CHECK(m.get32(0x00004040u) == 0xCAFEF00Eu);
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00200440u, kProgN + 8);
  CHECK(m.in_abt());   /* vectored through the DABORT handler    */
  CHECK((m.cpsr() & 0x1Fu) == MODE_ABT);
  CHECK(m.fsr() == FS_PERM_PAGE);
  CHECK(m.fsr_dom() == 1u);
  CHECK(m.far() == 0x00200440u);                    /* FAR holds the MVA        */
  CHECK(m.get32(0x00004440u) == 0u);                /* nothing was written      */

  /* Tiny page through a fine table: 1 KiB granularity, one AP for the lot. */
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00400140u, kProgN);
  CHECK(m.mode() == MODE_SVC);
  CHECK(m.getr(2) == 0x0BADF00Eu);
  CHECK(m.get32(0x00014140u) == 0x0BADF00Eu);
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00400540u, kProgN + 8);
  CHECK(m.in_abt());   /* vectored through the DABORT handler    */              /* the next KiB is a new    */
  CHECK(m.fsr() == FS_TRANS_PAGE);                  /* entry of the fine table  */
  CHECK(m.far() == 0x00400540u);

  /* No descriptor at all: a section translation fault. */
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00300040u, kProgN + 8);
  CHECK(m.in_abt());   /* vectored through the DABORT handler    */
  CHECK(m.fsr() == FS_TRANS_SECT);
  CHECK(m.far() == 0x00300040u);

  /* The instruction stream is translated too.  Here only the data half of the
   * address space is mapped, so the core faults on its next fetch and never
   * reaches a handler - the class of exception is all that can be observed, and
   * FAR must stay alone because prefetch aborts do not update it (2.3.7). */
  {
    Emu f;
    f.map_l1(0x00F00000u, sect(0u, 0, 1));
    /* Budget beyond the faulting fetch: with only the 0x00F00000 section
     * mapped the vector fetch aborts too, and every such step retires one
     * instruction (see Cpu::prepare_fetch). */
    f.phase(kProg, kProgN, SCTLR_M, dacc_of(0, DOM_MANAGER), 0, 0x00F09040u, 12);
    CHECK(f.in_abt());
    CHECK(f.nfault(BRAIN_EXC_PABORT) >= 1u);
    CHECK(f.far() == 0u);
  }
}

/* Large pages: 64 KiB, four 16 KiB subpages with independent permissions, and
 * the 16 consecutive entries the TRM requires for them. */
void test_large_page() {
  std::printf("[large page]\n");
  Emu m;
  if (!m.e) return;

  const uint32_t dacc = dacc_of(0, DOM_MANAGER) | dacc_of(1, DOM_CLIENT);
  m.map_l1(0u, sect(0u, 0, 1));
  m.map_l1(0x00600000u, l1_coarse(L2, 1));
  const uint32_t desc = l2_large(0x00080000u, 0, 1, 0, 0);   /* KiB 16-31: RW */
  const uint32_t base = l2_index_coarse(0x00600000u);
  for (uint32_t i = 0; i < 16; ++i) m.map_l2(L2, base + i, desc);
  m.put32(0x00084040u, 0x0F0F0F0Fu);

  m.clean(kProg, kProgN, SCTLR_M, dacc, 0, 0x00604040u);      /* subpage 1     */
  CHECK(m.getr(2) == 0x0F0F0F10u);
  CHECK(m.get32(0x00084040u) == 0x0F0F0F10u);

  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00600040u, kProgN + 8);
  CHECK(m.in_abt());   /* vectored through the DABORT handler    */                          /* subpage 0     */
  CHECK(m.fsr() == FS_PERM_PAGE);
  CHECK(m.get32(0x00080040u) == 0u);

  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00608040u, kProgN + 8);
  CHECK(m.in_abt());   /* vectored through the DABORT handler    */                          /* subpage 2     */
  CHECK(m.far() == 0x00608040u);

  /* One descriptor covers 64 KiB, so the 17th entry is already a new page -
   * and repeating it 16 times is what makes the other entries agree. */
  m.phase(kProg, kProgN, SCTLR_M, dacc, 0, 0x00610040u, kProgN + 8);
  CHECK(m.in_abt());   /* vectored through the DABORT handler    */
  CHECK(m.fsr() == FS_TRANS_PAGE);
}

/* Domain access control (3-11) outranks the descriptor's AP bits, and AP plus
 * the ARMv3-compatible S/R bits (3-12) decide the rest. */
void test_domains_and_ap() {
  std::printf("[domains, AP]\n");
  const uint32_t va = 0x00F00040u, pa = 0x00100040u;

  /* Domain with no access: refused before the AP bits matter, and reported as a
   * domain fault with the offending domain number. */
  {
    Emu m;
    m.map_l1(0u, sect(0u, 0, 1));
    m.map_l1(0x00F00000u, sect(0x00100000u, 1, 3));
    m.put32(pa, 0x12345678u);
    m.phase(kProg, kProgN, SCTLR_M,
            dacc_of(0, DOM_MANAGER) | dacc_of(1, DOM_NO), 0, va, kProgN + 8);
    CHECK(m.in_abt());   /* vectored through the DABORT handler    */
    CHECK(m.fsr() == FS_DOMAIN_SECT);
    CHECK(m.fsr_dom() == 1u);
    CHECK(m.get32(pa) == 0x12345678u);
  }

  /* Manager domain: "accesses are not checked against the access permission
   * bits so a permission fault cannot be generated" - even with AP = 00. */
  {
    Emu m;
    m.map_l1(0u, sect(0u, 0, 1));
    m.map_l1(0x00F00000u, sect(0x00100000u, 0, 0));
    m.put32(pa, 0x0BADC0DEu);
    m.clean(kProg, kProgN, SCTLR_M, dacc_of(0, DOM_MANAGER), 0, va);
    CHECK(m.getr(2) == 0x0BADC0DFu);
    CHECK(m.get32(pa) == 0x0BADC0DFu);
  }

  /* Client domain, AP = 00, CP15 c1 S = 1: privileged read-only.  The load
   * retires, the store is a section permission fault, and the destination must
   * not have been torn on the way out. */
  {
    Emu m;
    m.map_l1(0u, sect(0u, 0, 1));
    m.map_l1(0x00F00000u, sect(0x00100000u, 1, 0));
    m.put32(pa, 0x11111111u);
    const uint32_t d = dacc_of(0, DOM_MANAGER) | dacc_of(1, DOM_CLIENT);
    m.phase(kProg, kProgN, SCTLR_M | SCTLR_S, d, 0, va, 5);   /* up to the load  */
    CHECK(m.mode() == MODE_SVC);
    CHECK(m.nfault(BRAIN_EXC_DABORT) == 0u);
    CHECK(m.getr(2) == 0x11111111u);
    m.phase(kProg, kProgN, SCTLR_M | SCTLR_S, d, 0, va, kProgN + 8);
    CHECK(m.in_abt());   /* vectored through the DABORT handler    */
    CHECK(m.fsr() == FS_PERM_SECT);
    CHECK(m.far() == va);
    CHECK(m.get32(pa) == 0x11111111u);
  }

  /* Alignment checking is CP15 c1[1] and applies whether the MMU is enabled or
   * not (3.5.1); the access must not happen at all. */
  {
    Emu m;
    m.put32(0x9000u, 0x88776655u);   /* every byte distinct                   */
    m.phase(kProg, kProgN, SCTLR_A, 0, 0, 0x9001u, kProgN + 8);
    CHECK(m.in_abt());   /* vectored through the DABORT handler    */
    CHECK(m.fsr() == FS_ALIGN);
    CHECK(m.far() == 0x9001u);
    CHECK(m.get32(0x9000u) == 0x88776655u);   /* the access never happened     */
    /* The same access with checking off is simply fine: the core walks the
     * bytes at the byte-granular address (0x9001..0x9004), which is visible in
     * the word below - not the ARMv5 "aligned word, rotated" variant. */
    m.phase(kProg, kProgN, 0, 0, 0, 0x9001u, kProgN);
    CHECK(m.mode() == MODE_SVC);
    CHECK(m.nfault(BRAIN_EXC_DABORT) == 0u);
    /* With checking off the access is performed at the byte-granular address,
     * so the store at 0x9001 rewrote bytes 1..4 of this word. */
    CHECK(m.getr(2) == 0x00887767u);          /* [0x9001..0x9004] + 1          */
    CHECK(m.get32(0x9000u) == 0x88776755u);   /* bytes 1..4 moved, byte 0 did not */
  }
}

/* The TLB caches the translation *and* the permission decision for one
 * privilege level; only an invalidate makes the core look at a rewritten
 * descriptor. */
void test_tlb_invalidate() {
  std::printf("[tlb invalidate]\n");
  Emu m(4u << 20);            /* two 1 MiB sections to repoint between          */
  if (!m.e) return;

  const uint32_t dacc = dacc_of(0, DOM_MANAGER) | dacc_of(1, DOM_CLIENT);
  m.map_l1(0u, sect(0u, 0, 1));
  m.map_l1(0x00F00000u, sect(0x00100000u, 1, 1));
  m.put32(0x00100040u, 0xAAAAAAAAu);
  m.put32(0x00300040u, 0xBBBBBBBBu);

  brain_reset(m.e);
  m.load(kProgTlbi, kProgTlbiN);
  m.setr(0, SCTLR_M);
  m.setr(1, L1);
  m.setr(2, dacc);
  m.setr(3, 0);
  m.setr(5, 0x00F00040u);
  /* One round of copy-without-invalidate: insn 7 is the TLBI and must stay
   * unexecuted here, or there is nothing left to be stale. */
  CHECK(m.go(7u) == 7u);
  CHECK(m.mode() == MODE_SVC);
  CHECK(m.get32(0x00100040u) == 0xAAAAAAAAu + 1u);

  /* Repoint the descriptor behind the TLB's back: the cached page keeps
   * answering, and the new one stays untouched, until the guest invalidates. */
  m.map_l1(0x00F00000u, sect(0x00300000u, 1, 1));   /* 1 MiB-aligned, again    */
  m.again(4, 3, 0x00F00040u);                     /* ldr add str, no tlbi      */
  CHECK(m.get32(0x00100040u) == 0xAAAAAAAAu + 2u);
  CHECK(m.get32(0x00300040u) == 0xBBBBBBBBu);

  m.again(4, 4, 0x00F00040u);                     /* ldr add str + tlbi        */
  CHECK(m.get32(0x00300040u) == 0xBBBBBBBBu);     /* still not reached           */
  CHECK(m.mode() == MODE_SVC);
  CHECK(m.get32(0x00100040u) == 0xAAAAAAAAu + 3u);
  CHECK(m.get32(0x00300040u) == 0xBBBBBBBBu);

  m.again(4, 3, 0x00F00040u);                     /* now the new mapping answers*/
  CHECK(m.getr(2) == 0xBBBBBBBCu);
  CHECK(m.get32(0x00300040u) == 0xBBBBBBBCu);
  CHECK(m.get32(0x00100040u) == 0xAAAAAAAAu + 3u);
}

/* FCSE (2.3.13): while the MMU is on, addresses below 32 MiB are shifted by
 * the register's [31:25] field, addresses at or above 32 MiB are not touched,
 * and with the MMU off there is no shift and no translation at all. */
void test_fcse() {
  std::printf("[fcse]\n");

  /* Only the shifted section is mapped, so a core that forgot the shift - or
   * applied it to data but not to instruction fetches - aborts instead of
   * copying.  The register holds the PID pre-shifted by 25 bits. */
  {
    Emu m;
    m.map_l1(0x06000000u, sect(0u, 0, 1));
    m.put32(0x9040u, 0x55667788u);
    m.clean(kProg, kProgN, SCTLR_M, dacc_of(0, DOM_MANAGER), 3u << 25,
            0x9040u);
    CHECK(m.getr(2) == 0x55667789u);
    CHECK(m.get32(0x9040u) == 0x55667789u);
  }

  /* A full-size address is not shifted, even with a PID active. */
  {
    Emu m;
    m.map_l1(0x06000000u, sect(0u, 0, 1));        /* code, data, vectors       */
    /* The section field is [31:20]: a 1 MiB-aligned physical base, so this
     * mapping owns the whole 1 MiB at 0x00100000. */
    m.map_l1(0x80000000u, sect(0x00100000u, 1, 1));
    m.put32(0x00100040u, 0x600D600Du);
    m.clean(kProg, kProgN, SCTLR_M,
            dacc_of(0, DOM_MANAGER) | dacc_of(1, DOM_CLIENT), 3u << 25,
            0x80000000u + 0x40u);
    CHECK(m.getr(2) == 0x600D600Eu);
    CHECK(m.get32(0x00100040u) == 0x600D600Eu);
  }

  /* With the MMU off there is neither translation nor FCSE, which is what lets
   * boot code poke an i.MX28 register at 0x80000000 before any table exists.
   * One 32-bit access must be one bus transaction. */
  {
    Emu m;
    g_dev = Dev{0x933u, 0, 0};
    CHECK(brain_map_device(m.e, 0x80000000u, 4096u, dev_read, dev_write, nullptr,
                           "mmio") == 0);
    const uint32_t prog[] = {kMcrPidR3, kLdrR2r5, kAddR2, kStrR2r5};
    m.phase(prog, 4, 0, 0, 5u << 25, 0x80000000u, 4);
    CHECK(m.mode() == MODE_SVC);
    CHECK(g_dev.reads == 1);
    CHECK(g_dev.writes == 1);
    CHECK(m.getr(2) == 0x934u);                    /* read from the device     */
    CHECK(g_dev.value == 0x934u);                  /* and written straight back*/
  }

  /* Same with a low address: the unshifted physical location has to answer. */
  {
    Emu m;
    m.put32(0x9040u, 0xDEADBEEFu);
    m.phase(kProg, kProgN, 0, 0, 3u << 25, 0x9040u, kProgN);
    CHECK(m.mode() == MODE_SVC);
    CHECK(m.getr(2) == 0xDEADBEF0u);
    CHECK(m.get32(0x9040u) == 0xDEADBEF0u);
  }

  /* The FCSE PID is a seven-bit field, held pre-shifted in [31:25]; the context
   * ID is a plain 32-bit register (2.3.13, tables 2-26/2-27).  The MMU stays
   * off here, so both writes are safe no-ops as far as addresses go. */
  {
    Emu m;
    const uint32_t prog[] = {kMcrPidR3, kMcrCtxR3, kMrcPidR1, kMrcCtxR2};
    m.phase(prog, 4, 0, 0, 0xFFFFFFFFu, 0, 4);
    CHECK(m.mode() == MODE_SVC);
    CHECK(m.getr(1) == 0xFE000000u);
    CHECK(m.getr(2) == 0xFFFFFFFFu);
    /* A fast context switch must not need a TLB flush: entries are tagged with
     * MVAs, so the translations stay valid for the new process too. */
    Emu s;
    s.map_l1(0x06000000u, sect(0u, 0, 1));
    s.map_l1(0x00100000u, sect(0x00100000u, 0, 1));   /* the data page, with PID 0 */
    s.map_l1(0u, sect(0u, 0, 1));   /* the code and the vectors, with PID 0     */
    s.put32(0x00109040u, 0x77777777u);
    const uint32_t two[] = {kMcrTtbR1, kMcrDaccR2, kMcrCtrlR0, kLdrR2r5, kAddR2,
                            kStrR2r5, kMcrPidR3, kLdrR2r5};
    brain_reset(s.e);
    s.load(two, 8);
    s.setr(0, SCTLR_M);
    s.setr(1, L1);
    s.setr(2, dacc_of(0, DOM_MANAGER));
    s.setr(3, 0u);                    /* PID 0: VA 0x001xxxxx region          */
    s.setr(5, 0x00109040u);
    CHECK(s.go(7) == 7);              /* through the PID write in insn 6      */
    CHECK(s.mode() == MODE_SVC);
    CHECK(s.getr(2) == 0x77777778u);
    s.setr(3, 3u << 25);              /* switch process, no invalidation      */
    s.again(6, 1, 0x00109040u);       /* mcr c13: the PID moves to 3          */
    CHECK(s.mode() == MODE_SVC);
    /* The same VA with a new PID is a different MVA; nothing in the TLB can
     * answer for it, and 0x06109040 has no descriptor.  Four insns: the fault
     * plus the three-instruction handler. */
    s.again(7, 4, 0x00109040u);
    CHECK(s.in_abt());
    CHECK(s.nfault(BRAIN_EXC_DABORT) == 1u);
    CHECK(s.fsr() == FS_TRANS_SECT);
    CHECK(s.far() == 0x06109040u);    /* FAR holds the MVA, not the VA        */
  }
}

/* CP15 register file: what reads return, and what writes stick. */
void test_cp15() {
  std::printf("[cp15]\n");
  Emu m;
  if (!m.e) return;

  /* ID code: implementer 0x41, variant 0, architecture 0x6 (ARMv5TEJ), part
   * number 0x926, revision 5 - table 2-5, i.e. the r0p5 this TRM documents. */
  const uint32_t id_prog[] = {kMrcIdR4, kLsr24R5};
  brain_reset(m.e);
  m.load(id_prog, 2);
  CHECK(m.go(2) == 2);
  CHECK(m.getr(4) == 0x41069265u);
  CHECK(m.getr(5) == 0x41u);

  /* Cache type: Ctype b1110, S = 1 (separate I/D), 16 KiB 4-way with 8-word
   * lines - table 2-10.  There is no TCM in this model, so that reads zero. */
  const uint32_t ct_prog[] = {kMrcCtypeR5, kMrcTcmR6};
  brain_reset(m.e);
  m.load(ct_prog, 2);
  CHECK(m.go(2) == 2);
  CHECK(m.getr(5) == 0x1D152152u);
  CHECK(m.getr(6) == 0u);

  /* Control register: the functional bits take effect, [31:19] read back what
   * was written (the TRM's read-modify-write note), [18], [16] and [6:3] read
   * as 1, [17] and [11:10] read as 0 - table 2-11. */
  const uint32_t c1_prog[] = {kMcrCtrlR0, kMrcCtrlR3};
  brain_reset(m.e);
  m.load(c1_prog, 2);
  m.setr(0, 0u);
  CHECK(m.go(2) == 2);
  CHECK(m.getr(3) == 0x00050078u);
  m.setr(0, 0x00080000u);                          /* inside the reserved field */
  CHECK(brain_set_reg(m.e, 15, CODE) == 0);
  CHECK(m.go(2) == 2);
  CHECK(m.getr(3) == 0x000D0078u);
  /* [17] and [11:10] read 0 whatever the guest writes; [18], [16] and [6:3]
   * read 1.  c1[M] itself is not set here on purpose: enabling the MMU without
   * a translation table would abort the very next fetch, and M is covered by
   * every other phase in this file. */
  m.setr(0, 0x00020000u | 0x00000C00u);            /* the SBZ fields           */
  CHECK(brain_set_reg(m.e, 15, CODE) == 0);
  CHECK(m.go(2) == 2);
  CHECK(m.getr(3) == 0x00050078u);
  CHECK((m.getr(3) & 0x000700F8u) == 0x00050078u);

  /* A prefetch abort reports through the IFSR (2.3.6: the instruction-side FSR
   * is for debug, and FAR is not written by prefetch aborts at all). */
  {
    Emu f;
    f.map_l1(0u, sect(0u, 0, 1));                  /* code + vectors mapped     */
    /* Turn the MMU on, then branch into a region with no descriptor at all. */
    const uint32_t prog[] = {kMcrTtbR1, kMcrDaccR2, kMcrCtrlR0, kMovPcR5};
    /* r0 = c1 (M only), r5 = the victim address: 0x00F00000 has no level-one
     * descriptor, so the *fetch* of the last insn aborts.  Four insns plus the
     * three-instruction handler. */
    f.phase(prog, 4, SCTLR_M, dacc_of(0, DOM_MANAGER), 0u, 0x00F00000u, 12);
    CHECK(f.in_abt());
    CHECK(f.nfault(BRAIN_EXC_PABORT) >= 1u);
    CHECK(f.fsr() == FS_TRANS_SECT);
    CHECK(f.far() == 0u);     /* prefetch aborts never update the FAR (3.3.1)  */
  }
}

/* Data endianness is CP15 c1[7] alone: CPSR.E selects the byte order inside a
 * big-endian word and must not flip data accesses by itself. */
void test_endianness() {
  std::printf("[endianness]\n");
  Emu m;
  if (!m.e) return;
  m.put16(0x9000u, 0x1234u);

  m.phase(kProgBe, kProgBeN, 0u, 0u, 0u, 0x9000u, kProgBeN, MODE_SVC | CPSR_E);
  CHECK(m.mode() == MODE_SVC);
  CHECK(m.getr(2) == 0x1234u);                     /* E set, B clear: LE       */

  m.phase(kProgBe, kProgBeN, SCTLR_B, 0u, 0u, 0x9000u, kProgBeN, MODE_SVC);
  CHECK(m.getr(2) == 0x3412u);                     /* B set: the halfword moves*/
}

}  // namespace

int main() {
  std::printf("brain-emu mmu tests: %s\n", brain_version_string());
  test_translation();
  test_large_page();
  test_domains_and_ap();
  test_tlb_invalidate();
  test_fcse();
  test_cp15();
  test_endianness();
  std::printf("%s: %d/%d checks passed\n", g_failed ? "FAILED" : "PASS",
              g_checks - g_failed, g_checks);
  return g_failed ? 1 : 0;
}
