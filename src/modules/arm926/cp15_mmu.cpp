/* brain-emu: ARM926EJ-S system control coprocessor (CP15) + MMU.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Models the ARM926EJ-S MMU as documented in ARM DDI 0198E chapter 3:
 *   - CP15 c2: 16 KiB-aligned translation table base,
 *   - CP15 c3: 16 domains x 2 bits of access control,
 *   - first level descriptors: invalid / coarse table / section / fine table,
 *   - second level: large 64 KiB (AP per 16 KiB subpage), small 4 KiB
 *     (AP per 1 KiB subpage), tiny 1 KiB (single AP),
 *   - execute-never for data pages when CP15 c1[X] is set (AP[0]==1),
 *   - FCSE (CP15 c13 FCSE PID) producing the MVA used for caches/TLB,
 *   - faults reported with the ARM926EJ-S FSR encoding (TRM table 3-9:
 *     status in [3:0], domain in [7:4], write in [10], level-2 in [11]).
 *
 * The caches are not modelled: the core always sees a coherent view of the
 * bus, so every CP15 c7 cache operation is an architectural no-op and no
 * explicit I-cache invalidation is required (self-modifying code is handled by
 * the JIT's store tracking instead, see src/jit/cache.cpp).
 */
#include <cstdio>
#include <cstring>

#include "arm926/cpu.h"
#include "arm926/ops.h"
#include "arm926/jit/jit.h"

namespace brain::arm926 {

/* --------------------------------------------------------- CP15 helpers -- */

static inline u32 fcse_mva(Cpu* c, u32 va) {
  /* TRM 2.3.13: the core's addresses below 32MB become A + (FCSE_PID x 32MB),
   * addresses at or above 32MB are not modified at all, and with the MMU
   * disabled no FCSE translation happens.  The register holds the seven-bit PID
   * already shifted, in bits [31:25] - a fast context switch therefore writes
   * `n :SHL: 25`, which is why the value is used verbatim as the offset.
   *
   * Real hardware fetches the two instructions after the write with the old
   * PID; that pipeline detail is not modelled (the change takes effect on the
   * next fetch), which only makes the core stricter than the silicon. */
  if (!c->mmu_on || (va & 0xFE000000u)) return va;
  return va + (c->cp15.c[C13_PROCID] & 0xFE000000u);
}

void Cpu::sync_cp15_caches() {
  const u32 c1 = cp15.c[C1_CTRL];
  mmu_on = (c1 & SCTLR_M) ? 1 : 0;
  /* TRM table 2-11: bit [7] B alone selects the endianness of the data
   * accesses.  CPSR.E only picks BE32 vs BE8 byte order within a word and,
   * for v5, does not by itself flip data accesses. */
  be_data = (c1 & SCTLR_B) ? 1 : 0;
  /* TRM 3.5.1: "alignment fault checking is enabled by the A bit in CP15 c1"
   * and nothing else; the CPSR A bit is reserved on this core. */
  align_chk = (c1 & SCTLR_A) ? 1 : 0;
  ttb = cp15.c[C2_TTB] & 0xFFFFC000u;
  domain = cp15.c[C3_DACC];
  priv = ((cpsr & CPSR_MODE_M) != MODE_USR) ? 1u : 0u;
  /* The identity fast path is only valid with the MMU off (and not while the
   * core is in BE8 mode, where the swap is handled in the slow path). */
  ram_fast = (!mmu_on && ram && ram_mask) ? 1u : 0u;
}

void Cpu::on_translation_env_changed(const char* why) {
  (void)why;
  ++trans_gen;               /* invalidates every TLB entry and every block
                              * that cached a translation decision          */
  tlb_flush_all();
  sync_cp15_caches();
  if (jit) jit->flush_all(why);
}

void Cpu::tlb_flush_all() {
  const u32 n = tlb_mask + 1;
  for (u32 i = 0; i < n; ++i) {
    tlb_d[i].vpn = TLB_FREE; tlb_d[i].attr = 0; tlb_d[i].host = nullptr;
    tlb_i[i].vpn = TLB_FREE; tlb_i[i].attr = 0; tlb_i[i].host = nullptr;
  }
  fetch_e = nullptr;
  fetch_e_vpn = TLB_FREE;
}

/* Invalidate the entries for one page.  `mva` is a *modified* virtual address:
 * TRM 2.3.13 says FCSE is deliberately not applied to entry-based cache and TLB
 * maintenance operands (VA = MVA there), and the entries themselves are tagged
 * with the MVA, so callers must pass the address the entry was tagged with. */
void Cpu::tlb_flush_page(u32 mva) {
  const u32 idx = (mva >> 12) & tlb_mask;
  tlb_d[idx].vpn = TLB_FREE; tlb_d[idx].host = nullptr; tlb_d[idx].attr = 0;
  tlb_i[idx].vpn = TLB_FREE; tlb_i[idx].host = nullptr; tlb_i[idx].attr = 0;
  if (fetch_e_vpn == (mva >> 12)) fetch_e = nullptr;
}

/* ---------------------------------------------------------------- faults -- */

/* Report a translation/permission fault with the right exception kind. */
static inline void report_fault(Cpu* c, u32 flags, u32 fs, u32 mva) {
  c->exc_fsr = fs;
  c->exc_far = mva;
  if (flags & MF_FETCH) c->raise_pabort(fs);
  else c->raise_dabort(fs, mva);
}

/* ------------------------------------------------------------- walking -- */

/* A table walk reads *physical* memory, bypassing the TLB exactly like the
 * hardware walker.  Returns false on a bus error (external abort). */
static bool walk_read(Cpu* c, u32 pa, u32* out) {
  u32 v = 0;
  for (u32 i = 0; i < 4; ++i) {
    u8 b;
    if (!c->bus->read_byte(pa + i, &b)) return false;
    v |= (u32)b << (i * 8);
  }
  *out = v;
  return true;
}

static inline u32 domain_acc(Cpu* c, u32 dom) {
  return (c->cp15.c[C3_DACC] >> (dom * 2)) & 3u;
}

/* ARM926EJ-S access permissions: the domain access control value (CP15 c3)
 * selects whether the descriptor AP bits are consulted at all, and AP + the
 * legacy ARMv3 S/R bits (CP15 c1 [9:8]) give the rights per privilege level.
 * TRM tables 3-11 and 3-12:
 *
 *   domain 00 no access | 01 client (AP checked) | 10 reserved (no access)
 *          | 11 manager (AP *not* checked, no permission fault possible)
 *   AP  S R  privileged / user      (S = c1[8], R = c1[9]; only used for AP=00)
 *   00  0 0  no access  / no access
 *   00  1 0  read-only  / no access
 *   00  0 1  read-only  / read-only
 *   00  1 1  Unpredictable - modelled as no access
 *   01  x x  read/write  / no access
 *   10  x x  read/write  / read-only
 *   11  x x  read/write  / read/write
 *
 * The rights are computed for the current privilege level: TlbEntry::priv is
 * part of the TLB tag, so an entry never gets reused by the other level.
 * *fault is set when the access is not allowed at all, which the callers
 * report as a section/page permission fault.
 */
static u32 perms_of(u32 ap, u32 dacc, u32 sr, bool priv, u32* fault) {
  *fault = 0;
  if (dacc == 0 || dacc == 2) return 0;   /* domain forbids: handled as a
                                           * domain fault by the caller     */
  u32 p = 0, u = 0;
  if (dacc == 3) {                        /* manager: no permission checks  */
    p = u = TLB_READ | TLB_WRITE;
  } else {                                /* client                         */
    switch (ap) {
      case 3: p = u = TLB_READ | TLB_WRITE; break;
      case 2: p = TLB_READ | TLB_WRITE; u = TLB_READ; break;
      case 1: p = TLB_READ | TLB_WRITE; break;
      default:
        if (sr == 1) p = TLB_READ;                   /* S: privileged RO    */
        else if (sr == 2) p = u = TLB_READ;          /* R: RO for everybody */
        break;                /* sr == 0 (no access) and sr == 3 (Unpredictable) */
      }
  }
  const u32 a = priv ? p : u;
  if (!a) *fault = 1;
  return a;
}

bool Cpu::mmu_translate(u32 va, u32 flags, TlbEntry* out) {
  const u32 mva = fcse_mva(this, va);
  const u32 vpn = mva >> 12;
  memset(out, 0, sizeof(*out));
  out->vpn = vpn;
  out->priv = priv;
  out->gen = trans_gen;

  /* ---------------- MMU disabled: PA = MVA, and MVA = VA ---------------
   * No FCSE and no translation: the TRM's MMU-off rule is that the address the
   * core issues reaches the bus unchanged, which is what lets boot code poke
   * i.MX28-style MMIO at 0x80000000 before any page table exists. */
  if (!mmu_on) {
    const u32 pa = mva;
    PageDesc* pd = bus->page(pa);
    if (!(pd->flags & (PG_RAM | PG_ROM | PG_DEVICE))) {
      if (cfg.serror_on_busfault)
        report_fault(this, flags, FS926_EXT_LINEFILL, mva);
      return false;
    }
    out->gpa = pa & 0xFFFFF000u;
    out->host = (pd->flags & (PG_RAM | PG_ROM)) ? pd->host : nullptr;
    u32 a = TLB_VALID | TLB_READ | TLB_WRITE;
    if (pd->flags & PG_ROM) a |= TLB_ROM, a &= ~TLB_WRITE;
    if (pd->flags & PG_DEVICE) a |= TLB_DEVICE;
    out->attr = a;
    return true;
  }

  /* ---------------- first level -------------------------------------- */
  u32 desc;
  if (!walk_read(this, ttb + (mva >> 20) * 4, &desc)) {
    report_fault(this, flags, FS926_EXT_TRANS_L1, mva);
    return false;
  }
  stats.walks++;

  const u32 type = desc & 3u;
  if (type == 0) {                       /* section translation fault        */
    report_fault(this, flags,
                 FS926_TRANS_SECTION | (((desc >> 5) & 0xF) << FSR_DOMAIN_SHIFT),
                 mva);
    return false;
  }

  const u32 dom = (desc >> 5) & 0xF;
  const u32 dacc = domain_acc(this, dom);
  if (dacc == 0 || dacc == 2) {          /* domain fault                     */
    report_fault(this, flags,
                 ((type != 2) ? FS926_DOMAIN_PAGE : FS926_DOMAIN_SECT) |
                     (dom << FSR_DOMAIN_SHIFT),
                 mva);
    return false;
  }

  u32 ap, pa, attr, fs_perm;

  const u32 sr = (cp15.c[C1_CTRL] >> 8) & 3u;   /* S and R, table 3-12     */

  if (type == 2) {
    /* ---- section (1 MiB): base [31:20], AP [11:10], domain [8:5] ---- */
    ap = (desc >> 10) & 3;
    pa = (desc & 0xFFF00000u) | (mva & 0x000FFFFF);
    fs_perm = FS926_PERM_SECT;
    u32 fault;
    attr = perms_of(ap, dacc, sr, priv != 0, &fault);
    if (fault) {
      report_fault(this, flags, fs_perm | (dom << FSR_DOMAIN_SHIFT), mva);
      return false;
    }
  } else {
    /* ---- coarse (type 1) or fine (type 3) page table ---- */
    const u32 base = desc & (type == 1 ? 0xFFFFFC00u : 0xFFFFF000u);
    const u32 idx = (type == 1) ? ((mva >> 12) & 0xFF) : ((mva >> 10) & 0x3FF);
    u32 pte;
    if (!walk_read(this, base + idx * 4, &pte)) {
      report_fault(this, flags,
                   FS926_EXT_TRANS_L2 | (dom << FSR_DOMAIN_SHIFT),
                   mva);
      return false;
    }
    const u32 kind = pte & 3u;
    if (kind == 0) {
      report_fault(this, flags,
                   FS926_TRANS_PAGE | (dom << FSR_DOMAIN_SHIFT), mva);
      return false;
    }
    /* second level (table 3-7): base [31:16]/[31:12]/[31:10], AP [11:4] for
     * large and small pages, AP [5:4] for tiny pages, C/B [3:2], type [1:0] */
    if (kind == 1) {           /* large page: 64 KiB, AP per 16 KiB subpage */
      const u32 sub = (mva >> 14) & 3;
      ap = (pte >> (4 + sub * 2)) & 3;
      pa = (pte & 0xFFFF0000u) | (mva & 0x0000FFFF);
    } else if (kind == 2) {    /* small page: 4 KiB, AP per 1 KiB subpage   */
      const u32 sub = (mva >> 10) & 3;
      ap = (pte >> (4 + sub * 2)) & 3;
      pa = (pte & 0xFFFFF000u) | (mva & 0x00000FFF);
    } else {                   /* tiny page: 1 KiB, single AP               */
      ap = (pte >> 4) & 3;
      pa = (pte & 0xFFFFFC00u) | (mva & 0x000003FF);
    }
    fs_perm = FS926_PERM_PAGE;
    u32 fault;
    attr = perms_of(ap, dacc, sr, priv != 0, &fault);
    if (fault) {
      report_fault(this, flags, fs_perm | (dom << FSR_DOMAIN_SHIFT), mva);
      return false;
    }
  }

  /* ---------------- permission check for this access ------------------- */
  const bool is_write = (flags & MF_WRITE) != 0;
  const bool is_swp = (flags & MF_SWP) != 0;
  /* Instruction fetches are not protected on this core - there is no
   * execute-never bit - so only the read/write rights matter for them. */
  if (!(flags & MF_FETCH)) {
    if (attr & TLB_PRIV) {
      if (!priv) {
        report_fault(this, flags, fs_perm | (dom << FSR_DOMAIN_SHIFT), mva);
        return false;
      }
    }
    if (is_swp) {
      if (!(attr & TLB_READ) || !(attr & TLB_WRITE)) {
        report_fault(this, flags, fs_perm | (dom << FSR_DOMAIN_SHIFT), mva);
        return false;
      }
    } else if (is_write) {
      if (!(attr & TLB_WRITE)) {
        report_fault(this, flags, fs_perm | (dom << FSR_DOMAIN_SHIFT), mva);
        return false;
      }
    } else if (!(attr & TLB_READ)) {
      report_fault(this, flags, fs_perm | (dom << FSR_DOMAIN_SHIFT), mva);
      return false;
    }
  }

  /* ---------------- physical page -> host mapping ---------------------- */
  out->attr = TLB_VALID | attr;
  out->gpa = pa & 0xFFFFF000u;
  PageDesc* pd = bus->page(pa);
  if (pd->flags & PG_DEVICE) {
    out->attr |= TLB_DEVICE;
    out->host = nullptr;
  } else if (pd->flags & (PG_RAM | PG_ROM)) {
    /* Only legal when the *whole* 4 KiB page maps contiguously, which holds
     * for page-aligned sections/pages.  The offset inside the page comes from
     * the MVA because translation is page-granular at this level. */
    out->host = pd->host;
    if (pd->flags & PG_ROM) out->attr |= TLB_ROM;
  } else {
    report_fault(this, flags,
                 FS926_EXT_LINEFILL | (dom << FSR_DOMAIN_SHIFT),
                 mva);
    return false;
  }
  return true;
}

TlbEntry* Cpu::tlb_lookup(u32 va, u32 flags) {
  const u32 mva = fcse_mva(this, va);
  const u32 vpn = mva >> 12;
  TlbEntry* t = (flags & MF_FETCH) ? &tlb_i[vpn & tlb_mask] : &tlb_d[vpn & tlb_mask];

  if (BRAIN_LIKELY(t->vpn == vpn && t->gen == trans_gen && t->priv == priv)) {
    const u32 a = t->attr;
    bool ok;
    if (flags & MF_FETCH) ok = true;   /* no XN on this core, see above  */
    else if (flags & MF_SWP) ok = (a & TLB_READ) && (a & TLB_WRITE) && !(a & TLB_PRIV && !priv);
    else if (flags & MF_WRITE) ok = (a & TLB_WRITE) && !(a & TLB_PRIV && !priv) && !(a & TLB_ROM);
    else ok = (a & TLB_READ) && !(a & TLB_PRIV && !priv);
    if (BRAIN_LIKELY(ok && (a & TLB_VALID))) { stats.tlb_hits++; return t; }
    /* The cached entry forbids this access: fall through to the walker, which
     * recomputes and reports the exact fault. */
  }
  stats.tlb_miss++;

  TlbEntry fresh;
  if (!mmu_translate(va, flags, &fresh)) return nullptr;   /* fault queued */
  *t = fresh;
  return t;
}

/* ----------------------------------------------------- slow access paths -- */

u32 Cpu::mem_read_slow(u32 addr, u32 size, u32 flags) {
  if (BRAIN_UNLIKELY(exectrap)) return 0;
  if (align_chk && !(flags & (MF_UNAL | MF_FETCH)) && (addr & (size - 1))) {
    /* TRM 3.3.1: the MVA of the aborted data access goes to FAR, and the
     * status is b00x1 with the domain field left invalid (table 3-9). */
    exc_fsr = FS926_ALIGN;
    exc_far = fcse_mva(this, addr);
    raise_dabort(FS926_ALIGN, fcse_mva(this, addr));
    return 0;
  }
  TlbEntry* e = tlb_lookup(addr, flags);
  if (BRAIN_UNLIKELY(!e)) return 0;
  u32 v = 0;
  if (e->host) {
    const u8* p = e->host + (addr & 0xFFF);
    if (size == 4) memcpy(&v, p, 4);
    else if (size == 2) v = (u32)p[0] | ((u32)p[1] << 8);
    else v = p[0];
    if (be_data && size == 2) v = be32_swap16(v);
    return v;
  }
  BusRegion* r = bus->page(e->gpa + (addr & 0xFFF))->region;
  if (r && r->read) {
    if (size == 4 && !be_data && (addr & 3) == 0) {
      return r->read(r->user, addr, 4);
    }
    for (u32 i = 0; i < size; ++i) v |= (r->read(r->user, addr + i, 1) & 0xFF) << (i * 8);
    if (be_data && size == 2) v = be32_swap16(v);
    return v;
  }
  if (cfg.serror_on_busfault)
    raise_dabort(FS926_EXT_LINEFILL, fcse_mva(this, addr));
  return 0;
}

void Cpu::mem_write_slow(u32 addr, u32 val, u32 size, u32 flags) {
  if (BRAIN_UNLIKELY(exectrap)) return;
  if (align_chk && !(flags & (MF_UNAL | MF_FETCH)) && (addr & (size - 1))) {
    exc_fsr = FS926_ALIGN;
    exc_far = fcse_mva(this, addr);
    raise_dabort(FS926_ALIGN, fcse_mva(this, addr));
    return;
  }
  TlbEntry* e = tlb_lookup(addr, flags | MF_WRITE);
  if (BRAIN_UNLIKELY(!e)) return;
  if (e->host) {
    u8* p = e->host + (addr & 0xFFF);
    if (be_data && size == 2) val = be32_swap16(val);
    if (size == 4) memcpy(p, &val, 4);
    else if (size == 2) { p[0] = (u8)val; p[1] = (u8)(val >> 8); }
    else p[0] = (u8)val;
    if (BRAIN_UNLIKELY(smc_watch)) bus->note_store(addr, size);
    return;
  }
  BusRegion* r = bus->page(e->gpa + (addr & 0xFFF))->region;
  if (r && r->write) {
    /* One transaction per *aligned* word only - mirroring the read path - so a
     * misaligned store reaches the device as ascending byte writes. */
    if (size == 4 && !be_data && (addr & 3) == 0) { r->write(r->user, addr, val, 4); return; }
    for (u32 i = 0; i < size; ++i)
      r->write(r->user, addr + i, (val >> (i * 8)) & 0xFF, 1);
    return;
  }
  if (cfg.serror_on_busfault)
    raise_dabort(FS926_EXT_LINEFILL, fcse_mva(this, addr));
}

/* ---------------------------------------------------------------- CP15 -- */

void Cpu::cp15_write(u32 crn, u32 opc1, u32 rd, u32 crm, u32 opc2) {
  (void)opc1;
  const u32 val = regs[rd];

  /* CP15 accesses from User mode are undefined (TRM 2.2.2). */
  if ((cpsr & CPSR_MODE_M) == MODE_USR) { raise_undef(insn); return; }

  switch (crn) {
    case 0: return;                       /* ID registers: read-only        */
    case 1:
      if (opc2 == 1) { cp15.c[C1_TEST] = val; return; }
      if (opc2 == 7) { cp15.c[C1_AUX] = val; return; }
      /* SBO fields stay 1 and SBZ fields stay 0, as Table 2-11 requires, so
       * the read-back of a read-modify-write sequence is stable. */
      cp15.c[C1_CTRL] = (val & ~SCTLR_C1_SBZ) | SCTLR_C1_SBO;
      on_translation_env_changed("sctlr");
      return;
    case 2:
      cp15.c[C2_TTB] = val & 0xFFFFC000u;
      on_translation_env_changed("ttb");
      return;
    case 3:
      cp15.c[C3_DACC] = val;
      on_translation_env_changed("dacc");
      return;
    case 4: return;                        /* reserved                       */
    case 5:
      if (opc2 == 0) cp15.c[C5_FSR] = val & 0xFFFF000Fu;
      else cp15.c[C5_IFSR] = val & 0xFFFF000Fu;
      return;
    case 6: cp15.c[C6_FAR] = val; return;
    case 7: return;                        /* cache ops: no-ops (see header) */
    case 8:
      /* TLB operations.  crm=7,opc2=0: invalidate all; crm=5/6: by MVA for
       * I/D TLB; crm=3/4: by ASID (we do not tag by ASID -> flush all).     */
      if (crm == 7 && opc2 == 0) tlb_flush_all();
      else if ((crm == 5 || crm == 6) && opc2 == 0) tlb_flush_page(val);
      else tlb_flush_all();
      if (jit) jit->on_tlb_flush();
      return;
    case 9: return;                        /* lockdown: accepted, ignored    */
    case 10: return;                       /* TLB lockdown: ignored           */
    case 13:
      /* Table 2-26/2-27: opcode 2 = 0 is the FCSE PID (seven bits, pre-shifted
       * into [31:25]), 1 is the 32-bit Context ID, anything else is not
       * allocated.  A PID change needs no TLB flush - the entries are tagged
       * with MVAs and stay valid - but translated JIT blocks baked the old
       * translation in, so they have to go. */
      if (opc2 == 0) cp15.c[C13_PROCID] = val & 0xFE000000u;
      else if (opc2 == 1) cp15.c[C13_CTXID] = val;
      else return;
      if (jit) jit->on_tlb_flush();
      return;
    case 15: cp15.c[C15_CFR] = val; return;
    default: return;
  }
}

u32 Cpu::cp15_read(u32 crn, u32 opc1, u32 crm, u32 opc2) {
  (void)opc1; (void)crm;
  if ((cpsr & CPSR_MODE_M) == MODE_USR) { raise_undef(insn); return 0; }
  switch (crn) {
    case 0:
      /* Table 2-5/2-10: only p15,0,c0,c0,{0,1,2} are allocated; anything else
       * reads as zero rather than echoing the ID code. */
      if (opc1 != 0 || crm != 0) return 0;
      if (opc2 == 1) return cp15.c[C0_CACHE_TYPE];
      if (opc2 == 2 || opc2 == 7) return cp15.c[C0_TCM_R];
      return cp15.c[C0_IDCODE];
    case 1:
      if (opc2 == 1) return cp15.c[C1_TEST];
      if (opc2 == 7) return cp15.c[C1_AUX];
      return cp15.c[C1_CTRL];
    case 2: return cp15.c[C2_TTB];
    case 3: return cp15.c[C3_DACC];
    case 5: return opc2 ? cp15.c[C5_IFSR] : cp15.c[C5_FSR];
    case 6: return cp15.c[C6_FAR];
    case 7: return 0;
    case 9: return 0;
    case 10: return cp15.c[C10_TS];
    case 13:
      if (opc2 == 1) return cp15.c[C13_CTXID];
      return opc2 == 0 ? cp15.c[C13_PROCID] : 0;
    case 15: return cp15.c[C15_CFR];
    default: return 0;
  }
}

/* ------------------------------------------------ instruction fetch ------ */

bool Cpu::prepare_fetch() {
  /* The RAM identity fast path needs no translation at all. */
  if (BRAIN_LIKELY(ram_fast && ((cur - ram_gbase) & ~ram_mask) == 0)) {
    fetch_e = nullptr;
    return true;
  }
  TlbEntry* e = tlb_lookup(cur & ~0xFFFu, MF_FETCH | MF_SZ32);
  /* A failed fetch takes the exception here and retires nothing; the caller
   * charges the step (see Cpu::fetch_and_execute). */
  if (BRAIN_UNLIKELY(exectrap)) {
    const u32 x = exectrap;
    exectrap = 0;
    take_exception(x);
    return false;
  }
  if (BRAIN_UNLIKELY(!e)) {
    raise_pabort(FS926_EXT_LINEFILL);
    take_exception(EXC_PABORT);
    return false;
  }
  fetch_e = e;
  fetch_e_vpn = (cur >> 12);
  return true;
}

}  // namespace brain::arm926
