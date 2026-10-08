/* The ARM926EJ-S module: the adapter between the machine core and the CPU.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Everything here is translation: the core asks in its own vocabulary (a
 * register index, an entry address, an option key) and this file answers in the
 * ARM's (the CPSR is register 16 and the SPSR 17, the stack pointer is banked,
 * bit 0 of an entry address means Thumb).  The processor itself is `Cpu`, and
 * this adapter adds no state to it beyond the config it was built with.
 */
#include "machines/arm926/arm926.h"

#include <cstdlib>
#include <cstring>

namespace brain::arm926 {

const CpuInfo& info() {
  static const CpuInfo kInfo = [] {
    CpuInfo i;
    i.id = kId;
    i.name = "ARM926EJ-S";
    i.isa = "ARMv5TEJ (armel)";
    i.description =
        "the i.MX28's CPU core: ARMv5TEJ with the ARM/Thumb instruction sets, "
        "CP15 + MMU, 16 registers, and a JIT for this host";
    i.reg_count = 16;
    i.pc_index = 15;
    /* 16 and 17 are not general registers: they are the status word and its
     * saved copy, which is exactly how the C API addresses them too. */
    i.status_index = 16;
    i.saved_status_index = 17;
    i.sp_index = 13;
    i.alternate_state = true;
    i.alternate_state_name = "Thumb";
    return i;
  }();
  return kInfo;
}

/* ---------------------------------------------------------------- Core ---- */

const CpuInfo& Core::info() const { return arm926::info(); }

bool Core::init(Bus& bus, const CpuOptions& opt) {
  CpuConfig c;                      /* every ARM926 knob at its documented default */
  c.ram_size = 0;                   /* the machine owns the address space           */
  c.use_jit = opt.use_jit;
  c.trace = opt.trace;
  c.cycle_model = opt.cycle_model;
  c.big_endian_data = opt.big_endian_data;
  c.high_vectors = opt.high_vectors;
  return cpu_.attach(bus, c);
}

void Core::reset() { cpu_.reset(); }

void Core::sync() { cpu_.sync_cp15_caches(); }

u64 Core::run_until(u64 max_insns, u64 max_cycles) {
  const u64 before = cpu_.insns;
  cpu_.run_until(max_insns, max_cycles);
  return cpu_.insns - before;
}

bool Core::stop_requested() const { return cpu_.stop_requested(); }

void Core::step() { cpu_.step_one(); }
void Core::request_stop() { cpu_.request_stop(); }
u64 Core::insns() const { return cpu_.insns; }
u64 Core::cycles() const { return cpu_.cycles; }

u32 Core::get_reg(u32 index) const {
  if (index < 16) return cpu_.regs[index];
  if (index == info().status_index) return cpu_.cpsr;
  if (index == info().saved_status_index) return cpu_.bank[cpu_.bank_idx].spsr;
  return 0;
}

bool Core::set_reg(u32 index, u32 value) {
  if (index < 15) {
    /* A raw poke, as the C API has always done it: the current bank and the
     * live register file, without touching the other modes' copies. */
    cpu_.regs[index] = value;
    cpu_.bank[cpu_.bank_idx].r[index] = value;
    return true;
  }
  if (index == 15) { set_pc(value & ~1u, (value & 1u) != 0); return true; }
  if (index == info().status_index) {
    /* Also raw, and deliberately so: a debugger has to be able to look at (and
     * restore) flag and mode bits the guest itself could not write.  Changing
     * the mode bits this way does not re-bank the registers, which is only
     * ever correct when the caller knows that. */
    cpu_.cpsr = value;
    cpu_.vflag = (value >> 28) & 1u;
    cpu_.thumb = (value >> 5) & 1u;
    cpu_.sync_cp15_caches();
    return true;
  }
  if (index == info().saved_status_index) {
    cpu_.bank[cpu_.bank_idx].spsr = value;
    return true;
  }
  return false;
}

u32 Core::pc() const { return cpu_.npc; }

void Core::set_pc(u32 addr, bool alternate_state) {
  cpu_.set_thumb(alternate_state ? 1 : 0);
  cpu_.npc = addr & ~1u;
  cpu_.cur = cpu_.npc;
  cpu_.regs[15] = cpu_.npc;
  cpu_.bank[cpu_.bank_idx].r[15] = cpu_.npc;
  cpu_.sync_cp15_caches();
}

void Core::set_entry(u32 addr, bool alternate_state, u32 stack) {
  set_pc(addr, alternate_state);
  if (!stack) return;
  /* A fresh program's stack, in the two banks a loader can honestly speak for:
   * the live one and the User/System bank the program will fall back into.  The
   * exception banks keep whatever they had (a boot ROM's own stacks, if any). */
  cpu_.regs[13] = stack;
  cpu_.bank[BANK_USR].r[13] = stack;
  cpu_.bank[BANK_SVC].r[13] = stack;
  cpu_.flush_bank();
  cpu_.load_bank(cpu_.bank_idx);
}

u32 Core::guest_read(u32 addr, u32 size) {
  switch (size) {
    case 1: return cpu_.read8(addr);
    case 2: return cpu_.read16(addr);
    default: return cpu_.read32(addr);
  }
}

void Core::guest_write(u32 addr, u32 value, u32 size) {
  switch (size) {
    case 1: cpu_.write8(addr, value); break;
    case 2: cpu_.write16(addr, value); break;
    default: cpu_.write32(addr, value); break;
  }
}

void Core::set_irq(int level) { cpu_.irq_line = level ? 1u : 0u; }
void Core::set_fiq(int level) { cpu_.fiq_line = level ? 1u : 0u; }
int Core::pending_exception() const { return (int)cpu_.exc; }
const CpuStats& Core::stats() const { return cpu_.stats; }

bool Core::enable_jit(bool on) { return cpu_.jit_enable(on); }
bool Core::jit_active() const { return cpu_.jit_enabled(); }

/* Options a caller can set by name, which is how a tool or a test reaches the
 * knobs the generic config deliberately does not carry.  Unknown keys are
 * refused rather than ignored, so a typo is a failure and not a silent default. */
bool Core::set_option(const char* key, const char* value) {
  if (!key) return false;
  auto num = [&](u32 dflt) -> u32 {
    return value && *value ? (u32)std::strtoul(value, nullptr, 0) : dflt;
  };
  auto on = [&]() -> bool {
    return !value || !*value || std::strcmp(value, "0") != 0;
  };
  if (!std::strcmp(key, "jazelle_trap")) { cpu_.cfg.jazelle_trap = on(); return true; }
  if (!std::strcmp(key, "strict_unpredictable")) {
    cpu_.cfg.strict_unpredictable = on(); return true;
  }
  if (!std::strcmp(key, "serror_on_busfault")) {
    cpu_.cfg.serror_on_busfault = on(); return true;
  }
  if (!std::strcmp(key, "tlb_bits")) { cpu_.cfg.tlb_bits = num(cpu_.cfg.tlb_bits); return true; }
  if (!std::strcmp(key, "max_block_insns")) {
    cpu_.cfg.max_block_insns = num(cpu_.cfg.max_block_insns); return true;
  }
  if (!std::strcmp(key, "jit_cache_mb")) {
    cpu_.cfg.jit_cache_mb = num(cpu_.cfg.jit_cache_mb); return true;
  }
  if (!std::strcmp(key, "jit_fast_mem")) { cpu_.cfg.jit_fast_mem = on(); return true; }
  if (!std::strcmp(key, "big_endian_data")) {
    /* The generic option is the same bit; keeping the key here too means a
     * script that sets it after init() is not surprised. */
    cpu_.cfg.big_endian_data = on();
    cpu_.be_data = on() ? 1u : 0u;
    return true;
  }
  return false;
}

/* ------------------------------------------------------------- the module -- */

namespace {

class Arm926Module final : public CpuModule {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo kMod = {kId, "ARM926EJ-S", ModuleKind::Cpu,
                                    "ARMv5TEJ core (the i.MX28's CPU), with MMU "
                                    "and an interpreter + JIT"};
    return kMod;
  }
  std::unique_ptr<CpuCore> create() const override {
    return std::unique_ptr<CpuCore>(new Core());
  }
};

}  // namespace

const CpuModule& module() {
  static const Arm926Module kModule;
  return kModule;
}

}  // namespace brain::arm926

namespace brain {
void register_arm926_module() {
  ModuleRegistry::instance().add_cpu(&arm926::module());
}
}  // namespace brain
