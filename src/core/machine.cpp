/* brain-emu -- the machine: bus + CPU module + devices.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This file is the whole of the core's behaviour: build the address space, pick
 * a CPU module by id, hand it the bus and the options, and then be the thing
 * that decides when it runs.  It contains no opcode, no register name and no
 * exception: everything a processor does is behind CpuCore, everything that is
 * mapped is behind Bus.
 */
#include "brainemu/core/machine.h"

#include <cstring>

#include "brainemu/device/device.h"

namespace brain {
namespace {

/* Formats the module id list as "arm926 (ARM926EJ-S), toy8 (toy-8)": what a
 * refused `cpu_id` needs to say to be useful. */
void format_cpu_ids(std::string* out) {
  register_builtin_modules();
  const ModuleRegistry& reg = ModuleRegistry::instance();
  out->clear();
  for (u32 i = 0; i < reg.cpu_count(); ++i) {
    const ModuleInfo& m = reg.cpu_by_index(i)->info();
    if (i) *out += ", ";
    *out += m.id;
    *out += " (";
    *out += m.name;
    *out += ")";
  }
}

}  // namespace

Machine::Machine() = default;

Machine::~Machine() {
  /* The CPU goes first: a module (the ARM926 one, for instance) keeps a
   * back-pointer to the bus for TLB and code-cache invalidation, and destroying
   * the bus under it would leave that dangling during its own teardown. */
  cpu_.reset();
}

bool Machine::init(const MachineConfig& cfg) {
  cfg_ = cfg;
  err_.clear();
  /* A module in a static archive is only linked in if something references it,
   * so the list of modules this build contains is a function the core calls. */
  register_builtin_modules();

  if (cfg_.ram_size && !bus_.map_ram(cfg_.ram_base, cfg_.ram_size,
                                     cfg_.ram_name ? cfg_.ram_name : "ram")) {
    err_ = "RAM window overlaps an existing region";
    return false;
  }
  if (!build_cpu(cfg_.cpu_id)) return false;
  reset();
  return true;
}

bool Machine::reinit() {
  /* A full rebuild: the CPU goes first (it holds pointers into the bus), then
   * the address space is released and rebuilt from the same configuration.  The
   * devices are re-attached by their owners, so only the pointers are dropped
   * here - the objects outlive the machine, as `add_device` requires. */
  cpu_.reset();
  devices_.clear();
  bus_.reset();
  return init(cfg_);
}

bool Machine::build_cpu(const char* id) {
  cpu_.reset();
  const ModuleRegistry& reg = ModuleRegistry::instance();
  const CpuModule* mod = reg.cpu_by_id(id);
  if (!mod) {
    /* An empty id means "the default", which is the first module registered
     * (the tree's primary target); a *wrong* id is a caller error and says so,
     * with the ids that would have worked. */
    if (id && *id) {
      std::string ids;
      format_cpu_ids(&ids);
      err_ = std::string("no CPU module named '") + id + "': this build has " + ids;
      return false;
    }
    mod = reg.cpu_by_index(0);
  }
  if (!mod) {
    err_ = "this build contains no CPU module";
    return false;
  }
  cpu_ = mod->create();
  if (!cpu_) {
    err_ = std::string("CPU module '") + mod->info().id + "' could not be created";
    return false;
  }
  CpuOptions opt;
  opt.use_jit = cfg_.use_jit;
  opt.trace = cfg_.trace;
  opt.cycle_model = cfg_.cycle_model;
  opt.big_endian_data = cfg_.big_endian_data;
  opt.high_vectors = cfg_.high_vectors;
  if (!cpu_->init(bus_, opt)) {
    err_ = std::string("CPU module '") + mod->info().id + "' failed to initialise";
    cpu_.reset();
    return false;
  }
  return true;
}

void Machine::reset() {
  bus_.reset_stats();
  for (Device* d : devices_) d->reset();
  if (cpu_) cpu_->reset();
  device_cycles_ = 0;
}

bool Machine::add_device(Device& d) {
  if (!d.attach(bus_)) {
    err_ = std::string("device '") + d.info().id +
           "' does not fit at 0x" + std::to_string(d.base());
    return false;
  }
  devices_.push_back(&d);
  return true;
}

u64 Machine::run_insns(u64 n) {
  if (!cpu_) return 0;
  cpu_->sync();
  const u64 before = cpu_->insns();
  cpu_->run_insns(n);
  const u64 retired = cpu_->insns() - before;
  const u64 cyc = cpu_->cycles() - device_cycles_;
  device_cycles_ = cpu_->cycles();
  if (cyc) for (Device* d : devices_) d->tick(cyc);
  return retired;
}

u64 Machine::run_cycles(u64 max_cycles) {
  if (!cpu_) return 0;
  cpu_->sync();
  const u64 before = cpu_->insns();
  cpu_->run_cycles(max_cycles);
  const u64 retired = cpu_->insns() - before;
  const u64 cyc = cpu_->cycles() - device_cycles_;
  device_cycles_ = cpu_->cycles();
  if (cyc) for (Device* d : devices_) d->tick(cyc);
  return retired;
}

void Machine::step() {
  if (!cpu_) return;
  cpu_->sync();
  cpu_->step();
  const u64 cyc = cpu_->cycles() - device_cycles_;
  device_cycles_ = cpu_->cycles();
  if (cyc) for (Device* d : devices_) d->tick(cyc);
}

void Machine::stop() {
  if (cpu_) cpu_->request_stop();
}

/* --------------------------------------------------------------- images -- */

int Machine::load_raw(const void* data, size_t size, u32 addr) {
  err_.clear();
  ImageInfo info;
  if (raw_load(bus_, data, size, addr, err_)) return -1;
  info.entry = addr;
  info.format = "raw";
  info.min = addr;
  info.max = addr + (u32)size;
  info.segments = 1;
  /* A flat image carries no entry state beyond the address: no stack, no
   * instruction-set bit.  Whatever the caller set up before the load stands. */
  if (cpu_) {
    cpu_->set_pc(addr, false);
    cpu_->sync();
  }
  return 0;
}

/* Point the processor at a freshly loaded image.  The stack is worth a word:
 * a statically linked program expects one, and the only sensible place for a
 * machine that has never run is the top of its RAM window.  The CPU module is
 * asked to seed it (see CpuCore::set_entry) because only the module knows what
 * a stack pointer is in its own state. */
void Machine::set_entry_from(const ImageInfo& img) {
  if (!cpu_) return;
  u32 stack = 0;
  const u32 top = cfg_.ram_base + cfg_.ram_size;
  if (cfg_.ram_size && top > img.max + 64u) stack = (top - 16u) & ~7u;
  cpu_->set_entry(img.entry, (img.entry & 1u) != 0, stack);
  cpu_->sync();
}

int Machine::load_elf32(const void* data, size_t size, ImageInfo* info) {
  err_.clear();
  ImageInfo local;
  if (elf32_load(bus_, data, size, &local, err_)) return -1;
  set_entry_from(local);
  if (info) *info = local;
  return 0;
}

int Machine::load_image(const void* data, size_t size, u32 addr, ImageInfo* info) {
  err_.clear();
  const Loader* l = ModuleRegistry::instance().loader_by_probe(data, (u64)size);
  if (l) {
    ImageInfo local;
    if (l->load(bus_, data, size, &local, err_)) return -1;
    set_entry_from(local);
    if (info) *info = local;
    return 0;
  }
  /* Nothing claimed it: a flat image at the address the caller gave, which is
   * what a boot ROM would hand a payload. */
  return load_raw(data, size, addr);
}

/* --------------------------------------------------------------- helpers -- */

const char* default_cpu_id() {
  register_builtin_modules();
  const CpuModule* m = ModuleRegistry::instance().cpu_by_index(0);
  return m ? m->info().id : "";
}

}  // namespace brain
