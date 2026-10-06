/* brain-emu -- public C API implementation.
 * SPDX-file-note: see LICENSE.
 *
 * Thin, checked wrapper around the core.  It owns no state of its own beyond
 * the last error string: the CPU struct here is exactly what the tests and the
 * tools drive, so the API can never drift from what the library actually does.
 */
#include "brainemu/brainemu.h"

#include <cstring>
#include <string>

#include "core/cpu.h"
#include "core/ops.h"
#include "loader/loader.h"

using brain::CPSR_T;
using brain::u8;
using brain::Cpu;
using brain::CpuConfig;
using brain::ImageInfo;
using brain::u32;

/* The handle.  The tag has to be at global scope so it is the same `struct
 * brain_emu` the C header forward-declares. */
struct brain_emu {
  Cpu cpu;
  CpuConfig cfg;
  ImageInfo info;
  std::string err;
};

namespace {

const char* kExcName[] = {"none",    "reset", "undefined instruction",
                          "supervisor call", "prefetch abort", "data abort",
                          "interrupt",       "fast interrupt"};

void set_err(brain_emu* e, const std::string& msg) { e->err = msg; }

}  // namespace

extern "C" {

const char* brain_version_string(void) { return "brain-emu 0.1 (ARM926EJ-S, ARMv5TEJ)"; }

brain_emu_t* brain_emu_create(const brain_config_t* cfg) {
  brain_emu* e = new (std::nothrow) brain_emu();
  if (!e) return nullptr;
  CpuConfig c;
  if (cfg) {
    if (cfg->ram_base) c.ram_base = cfg->ram_base;
    if (cfg->ram_size) c.ram_size = cfg->ram_size;
    c.use_jit = cfg->use_jit != 0;
    c.big_endian_data = cfg->big_endian_data != 0;
    c.high_vectors = cfg->high_vectors != 0;
    c.trace = cfg->trace != 0;
    if (!cfg->cycle_model) c.cycle_model = false;
  }
  e->cfg = c;
  if (!e->cpu.init(c)) {
    set_err(e, "cpu init failed (bad ram window?)");
    delete e;
    return nullptr;
  }
  e->cpu.reset();
  return e;
}

void brain_emu_destroy(brain_emu_t* e) { delete e; }

void brain_reset(brain_emu_t* e) {
  if (!e) return;
  e->cpu.reset();
  e->info = ImageInfo();
}

const char* brain_last_error(const brain_emu_t* e) {
  if (!e) return "null handle";
  return e->err.empty() ? "no error" : e->err.c_str();
}

int brain_load_raw(brain_emu_t* e, const void* data, size_t size, u32 addr,
                   int thumb) {
  if (!e) return -1;
  e->err.clear();
  if (brain::raw_load(&e->cpu, data, size, addr, e->err)) return -1;
  e->info.is_elf = false;
  e->info.entry = addr | (thumb ? 1u : 0u);
  e->info.min = addr;
  e->info.max = addr + (u32)size;
  e->info.segments = 1;
  /* Enter the payload the way a boot ROM would: PC at the load address, state
   * from bit 0 of that address. */
  e->cpu.thumb = thumb ? 1u : 0u;
  e->cpu.cpsr = (e->cpu.cpsr & ~CPSR_T) | (thumb ? CPSR_T : 0u);
  e->cpu.npc = addr & ~1u;
  e->cpu.cur = e->cpu.npc;
  e->cpu.regs[15] = e->cpu.npc;
  e->cpu.sync_cp15_caches();
  return 0;
}

int brain_load_elf32(brain_emu_t* e, const void* image, size_t size,
                     u32* entry) {
  if (!e) return -1;
  e->err.clear();
  ImageInfo info;
  if (brain::elf32_load(&e->cpu, image, size, &info, e->err)) return -1;
  e->info = info;
  e->cpu.thumb = (info.entry & 1u) ? 1u : 0u;
  e->cpu.cpsr = (e->cpu.cpsr & ~CPSR_T) | (e->cpu.thumb ? CPSR_T : 0u);
  e->cpu.npc = info.entry & ~1u;
  e->cpu.cur = e->cpu.npc;
  e->cpu.regs[15] = e->cpu.npc;
  /* A statically linked program expects a stack; give it one at the top of the
   * RAM window, 8-aligned, and leave the two words at [SP] zeroed so a start-up
   * that peeks at argv/NULL termination does not read uninitialised memory. */
  const u32 top = e->cfg.ram_base + e->cfg.ram_size;
  if (top > info.max + 64u) {
    const u32 sp = (top - 16u) & ~7u;
    e->cpu.regs[13] = sp;
    e->cpu.bank[0].r[13] = sp;
    e->cpu.bank[3].r[13] = sp;
  }
  e->cpu.sync_cp15_caches();
  if (entry) *entry = info.entry;
  return 0;
}

int brain_map_device(brain_emu_t* e, u32 base, u32 size, brain_bus_read_t rd,
                     brain_bus_write_t wr, void* user, const char* name) {
  if (!e) return -1;
  e->err.clear();
  if (!e->cpu.bus->map_device(base, size, rd, wr, user,
                              name ? name : "device")) {
    set_err(e, "device overlaps an existing region");
    return -1;
  }
  return 0;
}

uint64_t brain_run_insns(brain_emu_t* e, uint64_t n) {
  if (!e) return 0;
  e->cpu.sync_cp15_caches();
  const uint64_t before = e->cpu.insns;
  e->cpu.run_insns(n);
  return e->cpu.insns - before;
}

uint64_t brain_run_cycles(brain_emu_t* e, uint64_t max_cycles) {
  if (!e) return 0;
  e->cpu.sync_cp15_caches();
  const uint64_t before = e->cpu.insns;
  e->cpu.run(max_cycles);
  return e->cpu.insns - before;
}

int brain_step(brain_emu_t* e) {
  if (!e) return -1;
  e->cpu.sync_cp15_caches();
  e->cpu.step_one();
  return 0;
}

void brain_stop(brain_emu_t* e) {
  if (e) e->cpu.exit_req = 1;
}

u32 brain_get_reg(brain_emu_t* e, int which) {
  if (!e) return 0;
  if (which >= 0 && which < 16) return e->cpu.regs[which];
  if (which == BRAIN_REG_CPSR) return e->cpu.cpsr;
  if (which == BRAIN_REG_SPSR) return e->cpu.bank[e->cpu.bank_idx].spsr;
  return 0;
}

int brain_set_reg(brain_emu_t* e, int which, u32 value) {
  if (!e) return -1;
  if (which >= 0 && which < 15) {
    e->cpu.regs[which] = value;
    e->cpu.bank[e->cpu.bank_idx].r[which] = value;
    return 0;
  }
  if (which == 15) {
    /* The PC is not a plain register here: bit 0 selects the state and the
     * core tracks the address of the *next* instruction separately. */
    e->cpu.thumb = (value & 1u) ? 1u : 0u;
    e->cpu.cpsr = (e->cpu.cpsr & ~CPSR_T) | (e->cpu.thumb ? CPSR_T : 0u);
    e->cpu.npc = value & ~1u;
    e->cpu.cur = e->cpu.npc;
    e->cpu.regs[15] = e->cpu.npc;
    e->cpu.bank[e->cpu.bank_idx].r[15] = e->cpu.npc;
    e->cpu.sync_cp15_caches();
    return 0;
  }
  if (which == BRAIN_REG_CPSR) {
    /* Deliberately a raw poke: a debugger has to be able to look at (and
     * restore) flag and mode bits the guest itself could not write.  Changing
     * the mode bits this way does not re-bank the registers, which is only
     * ever correct when the caller knows what it is doing. */
    e->cpu.cpsr = value;
    e->cpu.vflag = (value >> 28) & 1u;
    e->cpu.thumb = (value >> 5) & 1u;
    e->cpu.sync_cp15_caches();
    return 0;
  }
  if (which == BRAIN_REG_SPSR) {
    e->cpu.bank[e->cpu.bank_idx].spsr = value;
    return 0;
  }
  return -1;
}

u32 brain_read8(brain_emu_t* e, u32 addr) { return e ? e->cpu.read8(addr) : 0; }
u32 brain_read16(brain_emu_t* e, u32 addr) {
  return e ? e->cpu.read16(addr) : 0;
}
u32 brain_read32(brain_emu_t* e, u32 addr) {
  return e ? e->cpu.read32(addr) : 0;
}
void brain_write8(brain_emu_t* e, u32 addr, u32 value) {
  if (e) e->cpu.write8(addr, value);
}
void brain_write16(brain_emu_t* e, u32 addr, u32 value) {
  if (e) e->cpu.write16(addr, value);
}
void brain_write32(brain_emu_t* e, u32 addr, u32 value) {
  if (e) e->cpu.write32(addr, value);
}

/* Peek/poke go straight to the host buffer behind a RAM or ROM page, so they
 * never reach a device hook and never take an exception: an inspector that
 * perturbs a FIFO or a "read clear" register is worse than no inspector.  A
 * read from somewhere that is not RAM reports 0, and a write outside writable
 * RAM is dropped -- both are the caller's bug to find, not the core's to
 * report through the guest's exception pins. */
static u8* peek_ptr(brain_emu* e, u32 addr, u32 size, bool write) {
  if (!e) return nullptr;
  u8* p = e->cpu.bus->host_ptr(addr);
  if (!p) return nullptr;
  if (write && !e->cpu.bus->is_writable(addr)) return nullptr;
  /* A single access must not straddle the page it was resolved on. */
  if ((addr & 0xFFFu) + size > 0x1000u) return nullptr;
  return p;
}

u32 brain_peek8(brain_emu_t* e, u32 addr) {
  const u8* p = peek_ptr(e, addr, 1, false);
  return p ? p[0] : 0u;
}
u32 brain_peek16(brain_emu_t* e, u32 addr) {
  const u8* p = peek_ptr(e, addr, 2, false);
  return p ? (u32)p[0] | ((u32)p[1] << 8) : 0u;
}
u32 brain_peek32(brain_emu_t* e, u32 addr) {
  const u8* p = peek_ptr(e, addr, 4, false);
  return p ? (u32)p[0] | ((u32)p[1] << 8) | ((u32)p[2] << 16) | ((u32)p[3] << 24)
           : 0u;
}
void brain_poke8(brain_emu_t* e, u32 addr, u32 value) {
  u8* p = peek_ptr(e, addr, 1, true);
  if (p) p[0] = (u8)value;
}
void brain_poke16(brain_emu_t* e, u32 addr, u32 value) {
  u8* p = peek_ptr(e, addr, 2, true);
  if (p) { p[0] = (u8)value; p[1] = (u8)(value >> 8); }
}
void brain_poke32(brain_emu_t* e, u32 addr, u32 value) {
  u8* p = peek_ptr(e, addr, 4, true);
  if (!p) return;
  p[0] = (u8)value; p[1] = (u8)(value >> 8);
  p[2] = (u8)(value >> 16); p[3] = (u8)(value >> 24);
}

int brain_read_mem(brain_emu_t* e, u32 addr, void* dst, size_t n) {
  if (!e || !dst) return -1;
  return e->cpu.bus->read_bytes(addr, dst, n) ? 0 : -1;
}

int brain_write_mem(brain_emu_t* e, u32 addr, const void* src, size_t n) {
  if (!e || !src) return -1;
  return e->cpu.bus->write_bytes(addr, src, n) ? 0 : -1;
}

int brain_pending_exception(const brain_emu_t* e) {
  return e ? (int)e->cpu.exc : 0;
}

const char* brain_exception_name(int exc) {
  if (exc < 0 || exc >= BRAIN_EXC_COUNT) return "out of range";
  return kExcName[exc];
}

void brain_get_stats(const brain_emu_t* e, brain_stats_t* out) {
  if (!e || !out) return;
  const brain::Stats& s = e->cpu.stats;
  std::memset(out, 0, sizeof(*out));
  out->insns = e->cpu.insns;
  out->cycles = e->cpu.cycles;
  out->insns_interp = s.insns_interp;
  out->insns_jit = s.insns_jit;
  out->blocks = s.blocks;
  out->links = s.links;
  out->flushes = s.flushes;
  out->smc_flushes = s.smc_flushes;
  out->tlb_hits = s.tlb_hits;
  out->tlb_miss = s.tlb_miss;
  out->walks = s.walks;
  out->page_faults = s.page_faults;
  out->code_bytes = s.code_bytes;
  out->branch_taken = s.branch_taken;
  out->branch_not = s.branch_not;
  for (int i = 0; i < BRAIN_EXC_COUNT && i < (int)brain::EXC_COUNT; ++i)
    out->exc[i] = s.exc[i];
}

}  /* extern "C" */
