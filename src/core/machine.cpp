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

#include "brainemu/core/monitor.h"
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
  wires_.clear();
  /* A module in a static archive is only linked in if something references it,
   * so the list of modules this build contains is a function the core calls. */
  register_builtin_modules();

  /* A board is a *preset*: it fills the layout first (devices, monitors, wires)
   * and whatever the caller set itself is applied on top below, so a board never
   * takes away an explicit choice.  The module *lists* are emptied before the
   * board fills them: copying the caller's config wholesale would leave its own
   * devices and wires in front of the board's (a device the board also has would
   * then lose its window to the caller's copy, and a wire would be added twice -
   * once by the copy, once by the merge below). */
  MachineConfig eff = cfg;
  eff.device_count = 0;
  eff.monitor_count = 0;
  eff.wire_count = 0;
  if (cfg.board && *cfg.board && !apply_board(cfg.board, &eff)) return false;
  if (cfg.cpu_id && *cfg.cpu_id) eff.cpu_id = cfg.cpu_id;
  if (cfg.ram_base) eff.ram_base = cfg.ram_base;
  if (cfg.ram_size != MachineConfig{}.ram_size) eff.ram_size = cfg.ram_size;
  /* The caller's own modules, after the board's.  The lists are fixed-size and
   * drop silently when full, so the count is checked *before* anything is
   * appended: a machine that quietly lacks the peripheral a caller asked for is
   * the kind of failure that shows up as a mysterious hang much later. */
  if (eff.device_count + cfg.device_count > MachineConfig::kMaxModules) {
    err_ = "too many device modules in the configuration (limit " +
           std::to_string(MachineConfig::kMaxModules) + ")";
    return false;
  }
  if (eff.monitor_count + cfg.monitor_count > MachineConfig::kMaxModules) {
    err_ = "too many monitor modules in the configuration (limit " +
           std::to_string(MachineConfig::kMaxModules) + ")";
    return false;
  }
  if (eff.wire_count + cfg.wire_count > MachineConfig::kMaxWires) {
    err_ = "too many wires in the configuration (limit " +
           std::to_string(MachineConfig::kMaxWires) + ")";
    return false;
  }
  for (u32 i = 0; i < cfg.device_count; ++i) eff.add_device(cfg.devices[i]);
  for (u32 i = 0; i < cfg.monitor_count; ++i) eff.add_monitor(cfg.monitors[i]);
  for (u32 i = 0; i < cfg.wire_count; ++i) eff.add_wire(cfg.wires[i]);
  cfg_ = eff;
  for (u32 i = 0; i < cfg_.wire_count; ++i) wires_.push_back(intern_wire(cfg_.wires[i]));

  if (cfg_.ram_size && !bus_.map_ram(cfg_.ram_base, cfg_.ram_size,
                                     cfg_.ram_name ? cfg_.ram_name : "ram")) {
    err_ = "RAM window overlaps an existing region";
    return false;
  }
  if (!build_cpu(cfg_.cpu_id)) return false;
  /* The rest of the machine, by id.  A device or a monitor that fails to build
   * fails the machine: a board that silently lacks a peripheral is worse than
   * one that does not start. */
  for (u32 i = 0; i < cfg_.device_count; ++i)
    if (!add_device_by_id(cfg_.devices[i])) return false;
  for (u32 i = 0; i < cfg_.monitor_count; ++i)
    if (!add_monitor_by_id(cfg_.monitors[i].id)) return false;
  /* Every wire is checked once now that the modules exist, so a typo is reported
   * at construction instead of at the first interrupt - and it stays *live*
   * afterwards (a device added later is picked up on the next level change, and
   * `add_wire` validates on the spot). */
  for (const WireSpec& w : wires_) {
    std::string why;
    u32 input = 0;
    if (!resolve_wire(w, &input, &why)) {
      err_ = why;
      return false;
    }
  }
  reset();
  return true;
}

bool Machine::apply_board(const char* id, MachineConfig* eff) {
  const BoardModule* b = ModuleRegistry::instance().board_by_id(id);
  if (!b) {
    err_ = std::string("no board module named '") + id + "' (have: " +
           ModuleRegistry::instance().ids(ModuleKind::Board) + ")";
    return false;
  }
  /* The board configures a fresh config; the caller's own choices are merged in
   * by `init` afterwards.  Options are never part of a preset. */
  b->configure(*eff);
  eff->board = b->info().id;      /* so the machine can say where it came from */
  return true;
}

bool Machine::reinit() {
  /* A full rebuild: the CPU goes first (it holds pointers into the bus), then
   * the address space is released and rebuilt from the same configuration.  A
   * device the *caller* owns is re-attached by its owner, so only the pointers
   * are dropped here - those objects outlive the machine, as `add_device`
   * requires.  One the machine created from a module id is released with it,
   * window and all, because `init` is about to build it again from the same
   * spec. */
  cpu_.reset();
  devices_.clear();
  monitors_.clear();
  owned_devices_.clear();
  owned_monitors_.clear();
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
  for (Monitor* m : monitors_) m->on_reset(*this);
}

bool Machine::add_device(Device& d) {
  /* The wiring hook goes on first: a device that drives a line from `on_attach`
   * (or during `attach`'s reset) is already connected. */
  install_wiring(d);
  d.on_attach(*this);
  if (!d.attach(bus_)) {
    err_ = std::string("device '") + d.info().id +
           "' does not fit at 0x" + std::to_string(d.base());
    return false;
  }
  devices_.push_back(&d);
  return true;
}

Device* Machine::add_device_by_id(const char* id, u32 base, bool has_base) {
  const DeviceModule* mod = ModuleRegistry::instance().device_by_id(id);
  if (!mod) {
    err_ = std::string("no device module named '") + (id ? id : "") +
           "' (have: " + ModuleRegistry::instance().ids(ModuleKind::Device) + ")";
    return nullptr;
  }
  std::unique_ptr<Device> d = mod->create(has_base ? base : mod->default_base());
  if (!d) {
    err_ = std::string("device module '") + id + "' created nothing";
    return nullptr;
  }
  Device* raw = d.get();
  if (!add_device(*raw)) {
    err_ = std::string("device '") + raw->info().id + "': " + err_;
    return nullptr;          /* add_device named the window it could not take */
  }
  owned_devices_.push_back(std::move(d));
  return raw;
}

/* ------------------------------------------------------------- wiring --- */

namespace {

/* The index of an interrupt output by name: null/"" is "the first one", which is
 * what `--wire DEV=irq` means for a device with a single line. */
int output_index(const Device& d, const char* name) {
  if (!name || !*name) return d.irq_count() ? 0 : -1;
  for (u32 i = 0; i < d.irq_count(); ++i) {
    const char* n = d.irq_name(i);
    if (n && !std::strcmp(n, name)) return (int)i;
  }
  return -1;
}

}  // namespace

void Machine::install_wiring(Device& d) { d.set_irq_notify(&Machine::deliver_irq, this); }

bool Machine::resolve_wire(const WireSpec& w, u32* input_index, std::string* why) {
  if (!w.device || !*w.device) {
    if (why) *why = "a wire needs a device id";
    return false;
  }
  Device* d = device_by_id(w.device);
  if (!d) {
    if (why)
      *why = std::string("wire '") + w.device + "." + (w.output ? w.output : "") +
             " -> " + (w.input ? w.input : "") + "': no device named '" + w.device +
             "' on this machine (have:";
      for (const Device* d : devices_)
        if (d->info().id) *why += std::string(" ") + d->info().id;
      *why += ")";
    return false;
  }
  if (output_index(*d, w.output) < 0) {
    if (why) {
      *why = std::string("wire '") + w.device + "." + (w.output ? w.output : "") +
             " -> " + (w.input ? w.input : "") + "': device '" + w.device +
             "' has no interrupt output named '" + (w.output ? w.output : "") +
             "' (have:";
      for (u32 i = 0; i < d->irq_count(); ++i) *why += std::string(" ") + d->irq_name(i);
      *why += ")";
    }
    return false;
  }
  if (!cpu_) {
    if (why) *why = "no CPU to wire to";
    return false;
  }
  const char* want = w.input && *w.input ? w.input : "irq";
  for (u32 i = 0; i < cpu_->input_count(); ++i) {
    const char* n = cpu_->input_name(i);
    if (n && !std::strcmp(n, want)) {
      if (input_index) *input_index = i;
      return true;
    }
  }
  if (why) {
    *why = std::string("wire '") + w.device + "." + (w.output ? w.output : "") +
           " -> " + want + "': the CPU module has no input named '" + want + "' (have:";
    for (u32 i = 0; i < cpu_->input_count(); ++i)
      *why += std::string(" ") + cpu_->input_name(i);
    *why += ")";
  }
  return false;
}

/* A copy of the wire whose names live in the machine.  Every entry point goes
 * through here (`init` for the config's wires, `add_wire` for the live ones), so
 * nothing in `wires_` ever points into a caller's temporary. */
WireSpec Machine::intern_wire(const WireSpec& w) {
  WireSpec out;
  wire_names_.push_back(w.device ? w.device : "");
  out.device = wire_names_.back().c_str();
  wire_names_.push_back(w.output ? w.output : "");
  out.output = wire_names_.back().c_str();
  wire_names_.push_back(w.input ? w.input : "");
  out.input = wire_names_.back().c_str();
  return out;
}

bool Machine::add_wire(const WireSpec& w) {
  std::string why;
  u32 input = 0;
  if (!resolve_wire(w, &input, &why)) {
    err_ = why;
    return false;
  }
  wires_.push_back(intern_wire(w));
  return true;
}

int Machine::wire_level(u32 i) const {
  if (i >= wires_.size()) return 0;
  const WireSpec& w = wires_[i];
  for (const Device* d : devices_) {
    if (d->info().id && w.device && !std::strcmp(d->info().id, w.device))
      return d->irq_level((u32)output_index(*d, w.output));
  }
  return 0;
}

void Machine::deliver_irq(void* user, Device& d, u32 output, int level) {
  Machine* m = static_cast<Machine*>(user);
  if (!m || !m->cpu_ || !d.info().id) return;
  for (const WireSpec& w : m->wires_) {
    if (!w.device || std::strcmp(w.device, d.info().id)) continue;
    if (output_index(d, w.output) != (int)output) continue;
    for (u32 i = 0; i < m->cpu_->input_count(); ++i) {
      const char* n = m->cpu_->input_name(i);
      const char* want = w.input && *w.input ? w.input : "irq";
      if (n && !std::strcmp(n, want)) {
        m->cpu_->set_input(i, level);
        break;
      }
    }
  }
}

void Machine::end_slice(u64 retired) {
  const u64 cyc = cpu_->cycles() - device_cycles_;
  device_cycles_ = cpu_->cycles();
  if (cyc) {
    for (Device* d : devices_) d->tick(cyc);
  }
  for (Monitor* m : monitors_) m->on_slice(*this, retired);
}

/* One `run` is a sequence of slices, not one call.  A device is ticked - and a
 * monitor told - at every slice boundary, so a timer that expires 200 cycles
 * into a 10000-cycle run raises its line at the right moment: a machine that
 * only ticked devices when the run *ended* would deliver every interrupt at
 * once, at the end, and a run that never ends would deliver none.  The
 * granularity is what a device sees as "now": small enough that a peripheral
 * observes time move, large enough that the per-slice virtual call is noise. */
static constexpr u64 kSliceInsns = 1024;
static constexpr u64 kSliceCycles = 2048;

u64 Machine::run_insns(u64 n) {
  if (!cpu_ || n == 0) return 0;
  const u64 before = cpu_->insns();
  while (cpu_->insns() - before < n) {
    const u64 left = n - (cpu_->insns() - before);
    const u64 slice = left < kSliceInsns ? left : kSliceInsns;
    const u64 insns_before = cpu_->insns();
    cpu_->sync();
    cpu_->run_insns(slice);
    const u64 retired_slice = cpu_->insns() - insns_before;
    end_slice(retired_slice);
    /* The CPU retires fewer instructions than it was asked for only when it
     * stopped (`Machine::stop`, a device's request, a debugger): stop slicing. */
    if (retired_slice < slice) break;
  }
  return cpu_->insns() - before;
}

u64 Machine::run_cycles(u64 max_cycles) {
  if (!cpu_) return 0;
  const u64 before = cpu_->insns();
  const u64 target = cpu_->cycles() + max_cycles;
  for (;;) {
    const u64 now = cpu_->cycles();
    if (max_cycles && now >= target) break;
    const u64 slice = max_cycles && target - now < kSliceCycles ? target - now
                                                                : kSliceCycles;
    const u64 insns_before = cpu_->insns();
    cpu_->sync();
    cpu_->run_cycles(slice);
    end_slice(cpu_->insns() - insns_before);
    if (cpu_->cycles() - now < slice) break;      /* the CPU stopped */
  }
  return cpu_->insns() - before;
}

void Machine::step() {
  if (!cpu_) return;
  cpu_->sync();
  const u64 before = cpu_->insns();
  cpu_->step();
  end_slice(cpu_->insns() - before);
}

void Machine::stop() {
  if (cpu_) cpu_->request_stop();
}

Device* Machine::device_by_id(const char* id) {
  if (!id || !*id) return nullptr;
  for (Device* d : devices_)
    if (d->info().id && !std::strcmp(d->info().id, id)) return d;
  return nullptr;
}

void Machine::add_monitor(Monitor& m) { monitors_.push_back(&m); }

Monitor* Machine::add_monitor_by_id(const char* id) {
  const MonitorModule* mod = ModuleRegistry::instance().monitor_by_id(id);
  if (!mod) {
    err_ = std::string("no monitor module named '") + (id ? id : "") +
           "' (have: " + ModuleRegistry::instance().ids(ModuleKind::Monitor) + ")";
    return nullptr;
  }
  std::unique_ptr<Monitor> m = mod->create();
  if (!m) {
    err_ = std::string("monitor module '") + id + "' created nothing";
    return nullptr;
  }
  Monitor* raw = m.get();
  monitors_.push_back(raw);
  owned_monitors_.push_back(std::move(m));
  return raw;
}

Monitor* Machine::monitor_by_id(const char* id) {
  if (!id || !*id) return nullptr;
  for (Monitor* m : monitors_)
    if (m->info().id && !std::strcmp(m->info().id, id)) return m;
  return nullptr;
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
