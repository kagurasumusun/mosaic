/* brain-emu -- public C API implementation.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Thin, checked translation of the core: a handle is a Machine plus the error
 * string the C API promises, and every call forwards.  Two things are worth
 * knowing while reading it:
 *
 *   * the API is CPU-agnostic - it asks the module for the register indices it
 *     needs and never mentions a CPSR, a Thumb bit or an opcode.  The only
 *     place the ARM module shows through is the BRAIN_REG_CPSR/BRAIN_REG_SPSR
 *     numbering, which is that module's numbering and is what a caller written
 *     against the ARM926 keeps working with;
 *   * the handle owns the devices the caller maps through `brain_map_device`,
 *     because the machine keeps pointers to them and they have to outlive it.
 */
#include "brainemu/brainemu.h"

#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include "brainemu/core/machine.h"
#include "brainemu/cpu/cpu.h"
#include "brainemu/core/monitor.h"
#include "brainemu/device/device.h"
#include "brainemu/loader/loader.h"

using brain::CpuInfo;
using brain::CpuStats;
using brain::Device;
using brain::FuncDevice;
using brain::ImageInfo;
using brain::Machine;
using brain::MachineConfig;
using brain::Monitor;
using brain::ModuleRegistry;
using brain::u32;
using brain::u8;

/* The handle.  The tag has to be at global scope so it is the same `struct
 * brain_emu` the C header forward-declares. */
struct brain_emu {
  Machine m;
  std::vector<std::unique_ptr<Device>> devices;  /* owned; see the note above */
  std::string err;
};

namespace {

/* A create() that fails has no handle to hang its message on, and "cannot
 * create a machine" is exactly the kind of failure a caller needs the reason
 * for (an unknown `cpu_id` names the ids that would have worked).  So the
 * failure message goes here and `brain_last_error(nullptr)` returns it. */
std::string g_create_error;

const char* kExcName[] = {"none",    "reset", "undefined instruction",
                          "supervisor call", "prefetch abort", "data abort",
                          "interrupt",       "fast interrupt"};

void set_err(brain_emu* e, const std::string& msg) {
  if (e) {
    e->err = msg;
  } else {
    g_create_error = msg;
  }
}

/* "key=value;key=value" -> one set_option per pair.  Returns the count the
 * module refused, so a typo in a CPU option is reported and not swallowed. */
int apply_options(brain_emu* e, const char* spec) {
  int refused = 0;
  if (!spec || !*spec) return 0;
  std::string s(spec);
  size_t pos = 0;
  while (pos <= s.size()) {
    size_t semi = s.find(';', pos);
    if (semi == std::string::npos) semi = s.size();
    std::string item = s.substr(pos, semi - pos);
    pos = semi + 1;
    if (item.empty()) continue;
    const size_t eq = item.find('=');
    const std::string key = eq == std::string::npos ? item : item.substr(0, eq);
    const std::string val = eq == std::string::npos ? "" : item.substr(eq + 1);
    if (!e->m.cpu().set_option(key.c_str(), val.c_str())) {
      ++refused;
      set_err(e, "CPU module '" + std::string(e->m.cpu_info().id) +
                     "' does not know the option '" + key + "'");
    }
  }
  return refused;
}

}  // namespace

extern "C" {

const char* brain_version_string(void) {
  return "brain-emu 0.1 (core + modules; ARM926EJ-S, ARMv5TEJ)";
}

/* ---- modules ------------------------------------------------------------ */

u32 brain_module_count(void) {
  brain::register_builtin_modules();
  return ModuleRegistry::instance().count();
}

static const brain::ModuleInfo* mod_at(u32 index) {
  brain::register_builtin_modules();
  return ModuleRegistry::instance().info(index);
}

const char* brain_module_id(u32 index) {
  const brain::ModuleInfo* m = mod_at(index);
  return m ? m->id : "";
}

const char* brain_module_name(u32 index) {
  const brain::ModuleInfo* m = mod_at(index);
  return m ? m->name : "";
}

const char* brain_module_kind(u32 index) {
  const brain::ModuleInfo* m = mod_at(index);
  return m ? brain::module_kind_name(m->kind) : "";
}

const char* brain_module_description(u32 index) {
  const brain::ModuleInfo* m = mod_at(index);
  return m ? m->description : "";
}

const char* brain_default_cpu_id(void) { return brain::default_cpu_id(); }

/* ---- the CPU of a machine ----------------------------------------------- */

const char* brain_cpu_id(const brain_emu_t* e) {
  return e && e->m.has_cpu() ? e->m.cpu_info().id : "";
}
const char* brain_cpu_name(const brain_emu_t* e) {
  return e && e->m.has_cpu() ? e->m.cpu_info().name : "";
}
const char* brain_cpu_isa(const brain_emu_t* e) {
  return e && e->m.has_cpu() ? e->m.cpu_info().isa : "";
}
u32 brain_cpu_reg_count(const brain_emu_t* e) {
  return e && e->m.has_cpu() ? e->m.cpu_info().reg_count : 0;
}

static int reg_index(const brain_emu_t* e, u32 CpuInfo::*field) {
  if (!e || !e->m.has_cpu()) return -1;
  const u32 i = e->m.cpu_info().*field;
  return i == brain::CPU_NO_REG ? -1 : (int)i;
}

int brain_cpu_pc_reg(const brain_emu_t* e) { return reg_index(e, &CpuInfo::pc_index); }
int brain_cpu_status_reg(const brain_emu_t* e) {
  return reg_index(e, &CpuInfo::status_index);
}
int brain_cpu_saved_status_reg(const brain_emu_t* e) {
  return reg_index(e, &CpuInfo::saved_status_index);
}
int brain_cpu_sp_reg(const brain_emu_t* e) { return reg_index(e, &CpuInfo::sp_index); }
int brain_cpu_alternate_state(const brain_emu_t* e) {
  return e && e->m.has_cpu() && e->m.cpu_info().alternate_state ? 1 : 0;
}

int brain_set_option(brain_emu_t* e, const char* key, const char* value) {
  if (!e || !key) return 0;
  return e->m.cpu().set_option(key, value) ? 1 : 0;
}

/* ---- lifecycle ---------------------------------------------------------- */

brain_emu_t* brain_emu_create(const brain_config_t* cfg) {
  brain_emu* e = new (std::nothrow) brain_emu();
  if (!e) return nullptr;
  MachineConfig c;
  c.ram_base = 0;
  c.ram_size = 32u << 20;
  if (cfg) {
    if (cfg->ram_base) c.ram_base = cfg->ram_base;
    if (cfg->ram_size) c.ram_size = cfg->ram_size;
    c.cpu_id = cfg->cpu_id;
    c.board = cfg->board_id;
    c.use_jit = cfg->use_jit != 0;
    c.big_endian_data = cfg->big_endian_data != 0;
    c.high_vectors = cfg->high_vectors != 0;
    c.trace = cfg->trace != 0;
    c.cycle_model = cfg->cycle_model != 0;
  }
  if (!e->m.init(c)) {
    const std::string why = e->m.last_error();
    delete e;
    set_err(nullptr, why);
    return nullptr;
  }
  if (cfg && apply_options(e, cfg->cpu_options)) {
    /* The options are applied before the machine runs, so a refused key is a
     * creation failure: a caller that typed a knob expects it to be in effect. */
    const std::string why = e->err;
    delete e;
    set_err(nullptr, why);
    return nullptr;
  }
  g_create_error.clear();
  return e;
}

void brain_emu_destroy(brain_emu_t* e) { delete e; }

void brain_reset(brain_emu_t* e) {
  if (!e) return;
  e->m.reset();
  e->err.clear();
}

const char* brain_last_error(const brain_emu_t* e) {
  if (!e) return g_create_error.empty() ? "null handle" : g_create_error.c_str();
  if (!e->err.empty()) return e->err.c_str();
  return e->m.err().empty() ? "no error" : e->m.err().c_str();
}

/* ---- loading ------------------------------------------------------------ */

int brain_load_raw(brain_emu_t* e, const void* data, size_t size, u32 addr,
                   int thumb) {
  if (!e) return -1;
  e->err.clear();
  if (e->m.load_raw(data, size, addr)) {
    set_err(e, e->m.last_error());
    return -1;
  }
  /* Enter the payload the way a boot ROM would: PC at the load address, state
   * from bit 0 of that address. */
  e->m.cpu().set_pc(addr, thumb != 0);
  e->m.cpu().sync();
  return 0;
}

int brain_load_elf32(brain_emu_t* e, const void* image, size_t size,
                     u32* entry) {
  if (!e) return -1;
  e->err.clear();
  ImageInfo info;
  if (e->m.load_elf32(image, size, &info)) {
    set_err(e, e->m.last_error());
    return -1;
  }
  if (entry) *entry = info.entry;
  return 0;
}

int brain_add_device_module(brain_emu_t* e, const char* id, u32 base,
                            int has_base) {
  if (!e || !id) return -1;
  e->err.clear();
  if (!e->m.add_device_by_id(id, base, has_base != 0)) {
    set_err(e, e->m.last_error());
    return -1;
  }
  return 0;
}

int brain_add_device_named(brain_emu_t* e, const char* id, const char* name,
                           u32 base, int has_base) {
  if (!e || !id) return -1;
  e->err.clear();
  if (!e->m.add_device_by_id(id, base, has_base != 0, name)) {
    set_err(e, e->m.last_error());
    return -1;
  }
  return 0;
}

int brain_add_monitor_module(brain_emu_t* e, const char* id) {
  if (!e || !id) return -1;
  e->err.clear();
  if (!e->m.add_monitor_by_id(id)) {
    set_err(e, e->m.last_error());
    return -1;
  }
  return 0;
}

/* ---- wiring -------------------------------------------------------------- */

int brain_add_wire(brain_emu_t* e, const char* device_id, const char* output,
                   const char* cpu_input) {
  if (!e || !device_id) return -1;
  e->err.clear();
  if (!e->m.add_wire(brain::WireSpec(device_id, output, cpu_input))) {
    set_err(e, e->m.last_error());
    return -1;
  }
  return 0;
}

const char* brain_board_id(const brain_emu_t* e) {
  if (!e) return "";
  const char* b = e->m.config().board;
  return b ? b : "";
}

uint32_t brain_wire_count(const brain_emu_t* e) {
  return e ? e->m.wire_count() : 0;
}

int brain_wire_info(const brain_emu_t* e, uint32_t index, const char** device_id,
                    const char** output, const char** cpu_input) {
  if (!e || index >= e->m.wire_count()) return -1;
  const brain::WireSpec& w = e->m.wire(index);
  if (device_id) *device_id = w.device ? w.device : "";
  if (output) *output = w.output ? w.output : "";
  if (cpu_input) *cpu_input = w.input ? w.input : "";
  return 0;
}

int brain_wire_level(const brain_emu_t* e, uint32_t index) {
  return e ? e->m.wire_level(index) : 0;
}

/* ---- what the machine is made of ---------------------------------------- */

u32 brain_device_count(const brain_emu_t* e) {
  return e ? e->m.device_count() : 0;
}

const char* brain_device_instance(const brain_emu_t* e, u32 index) {
  const Device* d = e ? e->m.device(index) : nullptr;
  return (d && d->instance()) ? d->instance() : "";
}

const char* brain_device_id(const brain_emu_t* e, u32 index) {
  const Device* d = e ? e->m.device(index) : nullptr;
  return d ? d->info().id : "";
}

u32 brain_device_base(const brain_emu_t* e, u32 index) {
  const Device* d = e ? e->m.device(index) : nullptr;
  return d ? d->base() : 0;
}

u32 brain_device_field_count(const brain_emu_t* e, u32 device) {
  const Device* d = e ? e->m.device(device) : nullptr;
  return d ? d->field_count() : 0;
}

const char* brain_device_field_name(const brain_emu_t* e, u32 device, u32 field) {
  const Device* d = e ? e->m.device(device) : nullptr;
  return d && field < d->field_count() ? d->field_name(field) : "";
}

const char* brain_device_field_value(const brain_emu_t* e, u32 device, u32 field) {
  const Device* d = e ? e->m.device(device) : nullptr;
  return d && field < d->field_count() ? d->field_value(field) : "";
}

u32 brain_monitor_count(const brain_emu_t* e) {
  return e ? e->m.monitor_count() : 0;
}

const char* brain_monitor_id(const brain_emu_t* e, u32 index) {
  const Monitor* m = e ? e->m.monitor(index) : nullptr;
  return m ? m->info().id : "";
}

u32 brain_monitor_field_count(const brain_emu_t* e, u32 monitor) {
  const Monitor* m = e ? e->m.monitor(monitor) : nullptr;
  return m ? m->field_count() : 0;
}

const char* brain_monitor_field_name(const brain_emu_t* e, u32 monitor, u32 field) {
  const Monitor* m = e ? e->m.monitor(monitor) : nullptr;
  return m && field < m->field_count() ? m->field_name(field) : "";
}

const char* brain_monitor_field_value(const brain_emu_t* e, u32 monitor, u32 field) {
  const Monitor* m = e ? e->m.monitor(monitor) : nullptr;
  return m && field < m->field_count() ? m->field_value(field) : "";
}

int brain_map_device(brain_emu_t* e, u32 base, u32 size, brain_bus_read_t rd,
                     brain_bus_write_t wr, void* user, const char* name) {
  if (!e) return -1;
  e->err.clear();
  /* `brain_bus_read_t` and the bus's own callback type are the same shape, so
   * the adapter is a reinterpretation and not a wrapper with state. */
  auto device = std::unique_ptr<FuncDevice>(new FuncDevice(
      name ? name : "device", name ? name : "device", base, size,
      reinterpret_cast<brain::BusReadFn>(rd),
      reinterpret_cast<brain::BusWriteFn>(wr), user));
  if (!e->m.add_device(*device)) {
    set_err(e, e->m.last_error());
    return -1;
  }
  e->devices.push_back(std::move(device));
  return 0;
}

/* ---- running ------------------------------------------------------------ */

uint64_t brain_run_insns(brain_emu_t* e, uint64_t n) {
  return e ? e->m.run_insns(n) : 0;
}

uint64_t brain_run_cycles(brain_emu_t* e, uint64_t max_cycles) {
  return e ? e->m.run_cycles(max_cycles) : 0;
}

int brain_step(brain_emu_t* e) {
  if (!e) return -1;
  e->m.step();
  return 0;
}

void brain_stop(brain_emu_t* e) {
  if (e) e->m.stop();
}

/* ---- state -------------------------------------------------------------- */

u32 brain_get_reg(brain_emu_t* e, int which) {
  if (!e || !e->m.has_cpu() || which < 0) return 0;
  return e->m.cpu().get_reg((u32)which);
}

int brain_set_reg(brain_emu_t* e, int which, u32 value) {
  if (!e || !e->m.has_cpu() || which < 0) return -1;
  return e->m.cpu().set_reg((u32)which, value) ? 0 : -1;
}

u32 brain_read8(brain_emu_t* e, u32 addr) {
  return e && e->m.has_cpu() ? e->m.cpu().guest_read(addr, 1) : 0;
}
u32 brain_read16(brain_emu_t* e, u32 addr) {
  return e && e->m.has_cpu() ? e->m.cpu().guest_read(addr, 2) : 0;
}
u32 brain_read32(brain_emu_t* e, u32 addr) {
  return e && e->m.has_cpu() ? e->m.cpu().guest_read(addr, 4) : 0;
}
void brain_write8(brain_emu_t* e, u32 addr, u32 value) {
  if (e && e->m.has_cpu()) e->m.cpu().guest_write(addr, value, 1);
}
void brain_write16(brain_emu_t* e, u32 addr, u32 value) {
  if (e && e->m.has_cpu()) e->m.cpu().guest_write(addr, value, 2);
}
void brain_write32(brain_emu_t* e, u32 addr, u32 value) {
  if (e && e->m.has_cpu()) e->m.cpu().guest_write(addr, value, 4);
}

/* Peek/poke go straight to the host buffer behind a RAM or ROM page, so they
 * never reach a device hook and never take an exception: an inspector that
 * perturbs a FIFO or a "read clear" register is worse than no inspector.  A
 * read from somewhere that is not RAM reports 0, and a write outside writable
 * RAM is dropped -- both are the caller's bug to find, not the core's to
 * report through the guest's exception pins.
 *
 * A poke still has to tell the bus: it is a write into guest memory that the
 * CPU's store path never sees, so without `note_store` a JIT block translated
 * from the bytes it overwrites would keep running.  That is the same rule the
 * bulk loader writes follow in `Bus::write_bytes`; a *read* has nothing to
 * report. */
static u8* peek_ptr(brain_emu* e, u32 addr, u32 size, bool write) {
  if (!e) return nullptr;
  brain::Bus& bus = e->m.bus();
  u8* p = bus.host_ptr(addr);
  if (!p) return nullptr;
  if (write && !bus.is_writable(addr)) return nullptr;
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
  if (!p) return;
  p[0] = (u8)value;
  e->m.bus().note_store(addr, 1);
}
void brain_poke16(brain_emu_t* e, u32 addr, u32 value) {
  u8* p = peek_ptr(e, addr, 2, true);
  if (!p) return;
  p[0] = (u8)value; p[1] = (u8)(value >> 8);
  e->m.bus().note_store(addr, 2);
}
void brain_poke32(brain_emu_t* e, u32 addr, u32 value) {
  u8* p = peek_ptr(e, addr, 4, true);
  if (!p) return;
  p[0] = (u8)value; p[1] = (u8)(value >> 8);
  p[2] = (u8)(value >> 16); p[3] = (u8)(value >> 24);
  e->m.bus().note_store(addr, 4);
}

int brain_read_mem(brain_emu_t* e, u32 addr, void* dst, size_t n) {
  if (!e || !dst) return -1;
  return e->m.bus().read_bytes(addr, dst, n) ? 0 : -1;
}

int brain_write_mem(brain_emu_t* e, u32 addr, const void* src, size_t n) {
  if (!e || !src) return -1;
  return e->m.bus().write_bytes(addr, src, n) ? 0 : -1;
}

/* Level-driven interrupt inputs.  The core samples these at every step boundary
 * (FIQ first) while the module's own mask bits gate them; there is no pulse, so
 * holding the line high keeps a serviced request pending until the caller drops
 * it - which is exactly the ARM926's nIRQ/nFIQ pin behaviour an embedded guest
 * expects. */
void brain_set_irq(brain_emu_t* e, int level) {
  if (e && e->m.has_cpu()) e->m.cpu().set_irq(level);
}
void brain_set_fiq(brain_emu_t* e, int level) {
  if (e && e->m.has_cpu()) e->m.cpu().set_fiq(level);
}

int brain_pending_exception(const brain_emu_t* e) {
  return e && e->m.has_cpu() ? e->m.cpu().pending_exception() : 0;
}

const char* brain_exception_name(int exc) {
  if (exc < 0 || exc >= BRAIN_EXC_COUNT) return "out of range";
  return kExcName[exc];
}

void brain_get_stats(const brain_emu_t* e, brain_stats_t* out) {
  if (!e || !out) return;
  std::memset(out, 0, sizeof(*out));
  if (!e->m.has_cpu()) return;
  const CpuStats& s = e->m.cpu().stats();
  out->insns = e->m.cpu().insns();
  out->cycles = e->m.cpu().cycles();
  out->insns_interp = s.insns_interp;
  out->insns_jit = s.insns_jit;
  out->blocks = s.blocks;
  out->flushes = s.flushes;
  out->smc_flushes = s.smc_flushes;
  out->tlb_hits = s.tlb_hits;
  out->tlb_miss = s.tlb_miss;
  out->walks = s.walks;
  out->page_faults = s.page_faults;
  out->code_bytes = s.code_bytes;
  out->branch_taken = s.branch_taken;
  out->branch_not = s.branch_not;
  /* The exception ids are the module's; the first BRAIN_EXC_COUNT of them are
   * the ARM926's, which is what the C header names. */
  for (int i = 0; i < BRAIN_EXC_COUNT; ++i) out->exc[i] = s.exc[i];
}

}  /* extern "C" */
