/* brain-emu -- the machine: bus + CPU module + devices.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This file is the whole of the core's behaviour: build the address space, pick
 * a CPU module by id, hand it the bus and the options, and then be the thing
 * that decides when it runs.  It contains no opcode, no register name and no
 * exception: everything a processor does is behind CpuCore, everything that is
 * mapped is behind Bus.
 */
#include "brainemu/machine.h"

#include <cstring>

#include "brainemu/monitor.h"
#include "brainemu/device.h"

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

/* A configuration is full of `const char*` a caller may have taken from a
 * temporary (`std::string::c_str()` in the C API, an option parser, a board that
 * built a spec on the fly), and the machine keeps its configuration for as long
 * as it lives - `reinit()` rebuilds from it and `config()` hands it back.  So
 * every string is copied here, into storage that dies with the machine.
 *
 * The storage is a *fresh* deque each time: `cfg_` is about to point into it, and
 * swapping in a new one is what keeps a machine that is rebuilt (reinit) from
 * growing a little more storage with every rebuild - the old one is released only
 * after nothing can still be reading it. */
void Machine::own_config(MachineConfig& cfg,
                         std::unique_ptr<std::deque<std::string>>* store) {
  std::unique_ptr<std::deque<std::string>> names(new std::deque<std::string>());
  auto own = [&names](const char* s) -> const char* {
    if (!s) return nullptr;
    names->push_back(s);
    return names->back().c_str();
  };
  cfg.ram_name = own(cfg.ram_name);
  cfg.cpu_id = own(cfg.cpu_id);
  cfg.board = own(cfg.board);
  for (u32 i = 0; i < cfg.device_count; ++i) {
    cfg.devices[i].id = own(cfg.devices[i].id);
    cfg.devices[i].as = own(cfg.devices[i].as);
  }
  for (u32 i = 0; i < cfg.monitor_count; ++i)
    cfg.monitors[i].id = own(cfg.monitors[i].id);
  for (u32 i = 0; i < cfg.wire_count; ++i) {
    cfg.wires[i].device = own(cfg.wires[i].device);
    cfg.wires[i].output = own(cfg.wires[i].output);
    cfg.wires[i].input = own(cfg.wires[i].input);
  }
  *store = std::move(names);
}

bool Machine::init(const MachineConfig& cfg) {
  /* Building a machine can happen more than once (that is what `reinit` is), so
   * `init` starts from nothing: the CPU first, because a CPU module holds
   * pointers into the bus and into the device it talks to; then the devices and
   * monitors the machine owns; then the address space itself.  The configuration
   * is *not* touched here: `reinit` passes `cfg_` to this function, so it has to
   * stay readable until the new one is ready. */
  cpu_.reset();
  devices_.clear();
  monitors_.clear();
  owned_devices_.clear();
  owned_monitors_.clear();
  wires_.clear();
  wire_names_.clear();
  instance_names_.clear();
  bus_.reset();
  device_cycles_ = 0;
  err_.clear();
  /* A module in a static archive is only linked in if something references it,
   * so the list of modules this build contains is a function the core calls. */
  register_builtin_modules();

  /* The request first, in storage the machine owns: `reinit` re-enters this
   * function with `req_cfg_`, and a caller's `reinit` must not depend on the
   * caller's own strings still being alive.  This is also the only place the
   * request is read, so a build can never see another build's expansion. */
  MachineConfig req = cfg;
  own_config(req, &req_names_);
  req_cfg_ = req;

  /* A board is a *preset*: it fills the layout first (devices, monitors, wires)
   * and whatever the caller set itself is applied on top below, so a board never
   * takes away an explicit choice.  The module *lists* are emptied before the
   * board fills them: copying the caller's config wholesale would leave its own
   * devices and wires in front of the board's (a device the board also has would
   * then lose its window to the caller's copy, and a wire would be added twice -
   * once by the copy, once by the merge below). */
  MachineConfig eff = req;
  eff.device_count = 0;
  eff.monitor_count = 0;
  eff.wire_count = 0;
  if (req.board && *req.board && !apply_board(req.board, &eff)) return false;
  if (req.cpu_id && *req.cpu_id) eff.cpu_id = req.cpu_id;
  if (req.ram_base) eff.ram_base = req.ram_base;
  if (req.ram_size != MachineConfig{}.ram_size) eff.ram_size = req.ram_size;
  /* The caller's own modules, after the board's.  The lists are fixed-size and
   * drop silently when full, so the count is checked *before* anything is
   * appended: a machine that quietly lacks the peripheral a caller asked for is
   * the kind of failure that shows up as a mysterious hang much later. */
  if (eff.device_count + req.device_count > MachineConfig::kMaxModules) {
    err_ = "too many device modules in the configuration (limit " +
           std::to_string(MachineConfig::kMaxModules) + ")";
    return false;
  }
  if (eff.monitor_count + req.monitor_count > MachineConfig::kMaxModules) {
    err_ = "too many monitor modules in the configuration (limit " +
           std::to_string(MachineConfig::kMaxModules) + ")";
    return false;
  }
  if (eff.wire_count + req.wire_count > MachineConfig::kMaxWires) {
    err_ = "too many wires in the configuration (limit " +
           std::to_string(MachineConfig::kMaxWires) + ")";
    return false;
  }
  for (u32 i = 0; i < req.device_count; ++i) eff.add_device(req.devices[i]);
  for (u32 i = 0; i < req.monitor_count; ++i) eff.add_monitor(req.monitors[i]);
  for (u32 i = 0; i < req.wire_count; ++i) eff.add_wire(req.wires[i]);
  own_config(eff, &cfg_names_);        /* from here on the strings are ours */
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
  cpu_irq_wired_ = false;
  for (const WireSpec& w : wires_) {
    std::string why;
    WireEnd sink;
    if (!resolve_wire(w, &sink, &why)) {
      err_ = why;
      return false;
    }
    if (!sink.device) cpu_irq_wired_ = true;   /* a wire into the CPU itself */
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
  /* From the *request*, not from `cfg_`: `cfg_` already has the board's preset
   * merged in, and rebuilding from it would merge it a second time. */
  return init(req_cfg_);
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
  /* A caller-owned device may have named itself; two devices answering to one
   * name would make every wire ambiguous, so that is refused here (the
   * module-built path checks the same thing, with the id as the default name). */
  if (instance_taken(d.instance(), &d)) {
    err_ = std::string("two devices are called '") + d.instance() +
           "': give one an instance name (Device::set_instance)";
    return false;
  }
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

Device* Machine::add_device_by_id(const char* id, u32 base, bool has_base,
                                  const char* as) {
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
  /* The instance name is set before anything else can look the device up, and it
   * is owned here (a caller may hand a temporary): a second timer is only
   * addressable because it has one. */
  if (as && *as) {
    instance_names_.push_back(as);
    raw->set_instance(instance_names_.back().c_str());
  }
  if (instance_taken(raw->instance())) {
    err_ = std::string("two devices are called '") + raw->instance() +
           "': give one an instance name (ModuleSpec::as)";
    return nullptr;
  }
  if (!add_device(*raw)) {
    err_ = std::string("device '") + raw->instance() + "': " + err_;
    return nullptr;          /* add_device named the window it could not take */
  }
  owned_devices_.push_back(std::move(d));
  return raw;
}

/* Is this instance name already in use by a device that is not `except`?  Two
 * devices that answer to one name make every wire and every `--stats` line
 * ambiguous, and the ambiguity would show up as the *wrong peripheral* being
 * serviced - so it is refused where it is created. */
bool Machine::instance_taken(const char* name, const Device* except) const {
  if (!name || !*name) return false;
  for (const Device* d : devices_)
    if (d != except && d->instance() && !std::strcmp(d->instance(), name)) return true;
  return false;
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

bool Machine::resolve_wire(const WireSpec& w, WireEnd* sink, std::string* why) {
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
        if (d->instance()) *why += std::string(" ") + d->instance();
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
  const std::string want = (w.input && *w.input) ? w.input : "irq";
  /* A dot in the sink names a *device* input; without one it names a CPU input.
   * One rule, no flag, and a message that says which it was. */
  const size_t dot = want.find('.');
  if (dot == std::string::npos) {
    if (!cpu_) {
      if (why) *why = "no CPU to wire to";
      return false;
    }
    for (u32 i = 0; i < cpu_->input_count(); ++i) {
      const char* n = cpu_->input_name(i);
      if (n && !std::strcmp(n, want.c_str())) {
        if (sink) { sink->device = nullptr; sink->index = i; }
        return true;
      }
    }
    if (why) {
      *why = std::string("wire '") + w.device + "." + (w.output ? w.output : "") +
             " -> " + want + "': the CPU module has no input named '" + want +
             "' (have:";
      for (u32 i = 0; i < cpu_->input_count(); ++i)
        *why += std::string(" ") + cpu_->input_name(i);
      *why += ")";
    }
    return false;
  }

  const std::string dev_name = want.substr(0, dot);
  const std::string in_name = want.substr(dot + 1);
  Device* t = device_by_id(dev_name.c_str());
  if (!t) {
    if (why) {
      *why = std::string("wire '") + w.device + "." + (w.output ? w.output : "") +
             " -> " + want + "': no device named '" + dev_name + "' (have:";
      for (const Device* d : devices_)
        if (d->instance()) *why += std::string(" ") + d->instance();
      *why += ")";
    }
    return false;
  }
  for (u32 i = 0; i < t->input_count(); ++i) {
    const char* n = t->input_name(i);
    if (n && !std::strcmp(n, in_name.c_str())) {
      if (sink) { sink->device = t; sink->index = i; }
      return true;
    }
  }
  if (why) {
    *why = std::string("wire '") + w.device + "." + (w.output ? w.output : "") +
           " -> " + want + "': device '" + t->instance() +
           "' has no interrupt input named '" + in_name + "' (have:";
    for (u32 i = 0; i < t->input_count(); ++i)
      *why += std::string(" ") + t->input_name(i);
    if (!t->input_count()) *why += " none - it has no inputs at all";
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
  WireEnd sink;
  if (!resolve_wire(w, &sink, &why)) {
    err_ = why;
    return false;
  }
  wires_.push_back(intern_wire(w));
  if (!sink.device) cpu_irq_wired_ = true;
  return true;
}

int Machine::wire_level(u32 i) const {
  if (i >= wires_.size()) return 0;
  const WireSpec& w = wires_[i];
  for (const Device* d : devices_) {
    if (d->instance() && w.device && !std::strcmp(d->instance(), w.device))
      return d->irq_level((u32)output_index(*d, w.output));
  }
  return 0;
}

/* One level change on `d`'s output `output`, delivered to whoever is wired to
 * it.  The source is resolved by *instance* (so two timers stay apart) and the
 * sink may be a device input, which is how a chain - peripheral to controller to
 * CPU - works without either peripheral knowing about the other.  A change
 * nothing is wired to is dropped: that is what lets a device be used alone. */
void Machine::deliver_irq(void* user, Device& d, u32 output, int level) {
  Machine* m = static_cast<Machine*>(user);
  if (!m || m->wires_.empty()) return;
  for (const WireSpec& w : m->wires_) {
    if (!w.device) continue;
    if (m->device_by_id(w.device) != &d) continue;
    if (output_index(d, w.output) != (int)output) continue;
    WireEnd sink;
    std::string why;
    if (!m->resolve_wire(w, &sink, &why)) continue;   /* refused at init; live wires too */
    m->deliver(sink, level);
  }
}

/* Hand one level to a resolved sink.  A device sink re-enters `deliver_irq` when
 * the device answers by driving its own output, which is the point - and also why
 * the depth is capped: a cycle in the wire graph (a controller wired back into
 * itself through another device) asked to oscillate would otherwise recurse until
 * the stack ran out.  A machine like that is a design error, and it is reported
 * once rather than silently ignored. */
void Machine::deliver(const WireEnd& sink, int level) {
  if (sink.device) {
    if (wiring_depth_ >= kMaxWiringDepth) {
      if (!wiring_depth_reported_) {
        wiring_depth_reported_ = true;
        std::fprintf(stderr, "[machine] wire graph oscillates past depth %u "
                             "('a' wired to 'b' wired back to 'a'?); dropping\n",
                     kMaxWiringDepth);
      }
      return;
    }
    ++wiring_depth_;
    sink.device->set_input(sink.index, level);
    --wiring_depth_;
    return;
  }
  if (cpu_) cpu_->set_input(sink.index, level);
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
 * into a 10000-cycle run raises its line at the right moment: a machine that only
 * ticked devices when the run *ended* would deliver every interrupt at once, at
 * the end, and a run that never ends would deliver none.
 *
 * A slice is bounded on *both* counters (`run_until`), which is what a guest in
 * a low-power state needs: it retires nothing at all while it waits, so an
 * instruction budget alone would never come back to the machine, the device that
 * is supposed to wake it would never be ticked, and the run would hang.  With a
 * cycle bound the halted CPU charges time, the slice ends, the devices see it.
 *
 * A slice is also the unit a monitor is told about, so a slice in which *nothing*
 * happened - no instruction retired and no cycle charged - is not one: it is a
 * CPU that cannot get anywhere (a handler whose vector is unmapped re-takes the
 * same abort for ever), and the run reports nothing and ends.  The granularity of
 * a slice is what a device sees as "now": small enough that a peripheral observes
 * time move, large enough that the per-slice virtual call is noise. */
static constexpr u64 kSliceInsns = 1024;
static constexpr u64 kSliceCycles = 2048;

u64 Machine::run_insns(u64 n) {
  if (!cpu_ || n == 0) return 0;
  const u64 before = cpu_->insns();
  while (cpu_->insns() - before < n) {
    const u64 left = n - (cpu_->insns() - before);
    const u64 slice = left < kSliceInsns ? left : kSliceInsns;
    const u64 insns_before = cpu_->insns(), cycles_before = cpu_->cycles();
    cpu_->sync();
    cpu_->run_until(slice, kSliceCycles);
    const u64 retired = cpu_->insns() - insns_before;
    const u64 charged = cpu_->cycles() - cycles_before;
    if (retired || charged) end_slice(retired);
    if (cpu_->stop_requested()) break;     /* a device, a monitor, a debugger  */
    if (!retired && !charged) break;       /* the CPU cannot get anywhere      */
    /* Retiring nothing while charging cycles is a CPU that is waiting (a low
     * power state): that is a wait for an interrupt, and it is only worth
     * waiting in a machine where one can arrive.  Without a wire into the CPU
     * nothing can raise a line, so the instruction budget can never be met and
     * the call would never end. */
    if (!retired && !cpu_irq_wired_) break;
  }
  return cpu_->insns() - before;
}

u64 Machine::run_cycles(u64 max_cycles) {
  if (!cpu_) return 0;
  const u64 before = cpu_->insns();
  u64 left = max_cycles;                            /* 0 = until stopped */
  while (!max_cycles || left) {
    const u64 slice = max_cycles && left < kSliceCycles ? left : kSliceCycles;
    const u64 insns_before = cpu_->insns(), cycles_before = cpu_->cycles();
    cpu_->sync();
    cpu_->run_until(kSliceInsns, slice);
    const u64 retired = cpu_->insns() - insns_before;
    const u64 charged = cpu_->cycles() - cycles_before;
    if (retired || charged) end_slice(retired);
    if (cpu_->stop_requested()) break;
    if (!retired && !charged) break;
    /* An engine charges *at least* the cycles it was asked for (an instruction
     * that costs three when one is left still runs), so the rest of the budget
     * is the request minus what was charged, and it can be nil. */
    left = charged < left ? left - charged : 0;
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
  /* Instance names win: that is what makes `t0` and `t1` two devices built from
   * the one `timer` module.  A module id still resolves when no instance claims
   * it, which is what lets a machine with a single console call it "console". */
  for (Device* d : devices_)
    if (d->instance() && !std::strcmp(d->instance(), id)) return d;
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
