/* toy8: the second CPU module's implementation.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The instruction set is in toy8.h.  What is worth reading here is how little
 * the core asks for: fetch a byte, execute it, keep the counters, and answer
 * register reads.  There is no MMU, no exception model, no JIT and no clock
 * beyond one cycle per instruction - and none of that is visible to the machine,
 * the bus, the devices or the C API, because none of them knows what a toy8 is.
 */
#include "machines/toy8/toy8.h"

#include <cstdio>
#include <cstring>

#include "brainemu/bus.h"

namespace brain::toy8 {
namespace {

struct State {
  u8 a = 0, x = 0, sp = 0, flags = 0;
  u16 pc = 0;
  bool halted = false;
  u64 insns = 0, cycles = 0;
};

class Core final : public CpuCore {
 public:
  const CpuInfo& info() const override;
  bool init(Bus& bus, const CpuOptions& opt) override;
  void reset() override;
  u64 run_until(u64 max_insns, u64 max_cycles) override;
  bool stop_requested() const override { return stop_; }
  void step() override;
  void request_stop() override;
  u64 insns() const override { return s_.insns; }
  u64 cycles() const override { return s_.cycles; }
  u32 get_reg(u32 index) const override;
  bool set_reg(u32 index, u32 value) override;
  u32 pc() const override { return s_.pc; }
  void set_pc(u32 addr, bool alternate_state) override;
  void set_entry(u32 addr, bool alternate_state, u32 stack) override;
  u32 guest_read(u32 addr, u32 size) override;
  void guest_write(u32 addr, u32 value, u32 size) override;
  void set_irq(int level) override { (void)level; }
  void set_fiq(int level) override { (void)level; }
  int pending_exception() const override { return 0; }
  const CpuStats& stats() const override { return stats_; }
  bool set_option(const char* key, const char* value) override;
  bool enable_jit(bool on) override { (void)on; return false; }
  bool jit_active() const override { return false; }

 private:
  u8 fetch8();
  u16 fetch16();
  void step_one();
  void set_flags(u8 value, bool carry);
  void push(u8 v);
  u8 pop();

  State s_{};
  Bus* bus_ = nullptr;
  bool trace_ = false;
  bool stop_ = false;
  CpuStats stats_{};
};

const CpuInfo& Core::info() const {
  static const CpuInfo kInfo = [] {
    CpuInfo i;
    i.id = "toy8";
    i.name = "toy-8";
    i.isa = "toy8-1 (this project's demo ISA)";
    i.description =
        "a five-register accumulator machine with a 16-bit address space: the "
        "worked example of a CPU module (not an emulator of real hardware)";
    i.reg_count = kRegCount;
    i.pc_index = kRegPC;
    i.status_index = kRegFlags;
    i.saved_status_index = CPU_NO_REG;
    i.sp_index = kRegSP;
    i.alternate_state = false;
    return i;
  }();
  return kInfo;
}

bool Core::init(Bus& bus, const CpuOptions& opt) {
  bus_ = &bus;
  trace_ = opt.trace;
  reset();
  return true;
}

void Core::reset() {
  s_ = State{};
  stop_ = false;
  /* A fresh machine begins with a stack at the top of the 16-bit space, which
   * is the closest thing this ISA has to a documented reset state. */
  s_.sp = 0xFF;
}

void Core::request_stop() { stop_ = true; }

u8 Core::fetch8() {
  u8 v = 0;
  if (!bus_->read_byte(s_.pc, &v)) v = 0;        /* unmapped reads as zero    */
  s_.pc = (u16)(s_.pc + 1);
  return v;
}

u16 Core::fetch16() {
  const u8 lo = fetch8();
  const u8 hi = fetch8();
  return (u16)(lo | ((u16)hi << 8));
}

/* Z and N always follow the value; C is whatever the operation produced (the
 * borrow of a subtraction, the carry of an addition, and for everything else
 * the bit that left the byte). */
void Core::set_flags(u8 value, bool carry) {
  u8 f = 0;
  if (value == 0) f |= kZ;
  if (value & 0x80u) f |= kN;
  if (carry) f |= kC;
  s_.flags = f;
}

void Core::push(u8 v) {
  bus_->write_byte((u16)(0x0100u | s_.sp), v);
  s_.sp = (u8)(s_.sp - 1);
}

u8 Core::pop() {
  s_.sp = (u8)(s_.sp + 1);
  u8 v = 0;
  bus_->read_byte((u16)(0x0100u | s_.sp), &v);
  return v;
}

void Core::step_one() {
  if (s_.halted || stop_) return;
  const u16 at = s_.pc;
  const u8 op = fetch8();
  switch (op) {
    case 0x00: break;                                       /* NOP           */
    case 0x01: s_.a = fetch8(); break;                      /* LDA #imm      */
    case 0x02: s_.x = fetch8(); break;                      /* LDX #imm      */
    case 0x03: { u16 a = fetch16(); bus_->write_byte(a, s_.a); break; }
    case 0x04: { u16 a = fetch16(); bus_->read_byte(a, &s_.a); break; }
    case 0x05: bus_->write_byte((u16)(0x0200u | s_.x), s_.a); break;
    case 0x06: bus_->read_byte((u16)(0x0200u | s_.x), &s_.a); break;
    case 0x07: { u16 t = (u16)s_.a + fetch8(); set_flags((u8)t, t > 0xFF); s_.a = (u8)t; break; }
    case 0x08: { u8 v = fetch8(); set_flags((u8)(s_.a - v), s_.a >= v); s_.a = (u8)(s_.a - v); break; }
    case 0x09: { u8 v = fetch8(); set_flags((u8)(s_.a - v), s_.a >= v); break; }
    case 0x0A: s_.a &= fetch8(); set_flags(s_.a, false); break;
    case 0x0B: s_.a |= fetch8(); set_flags(s_.a, false); break;
    case 0x0C: s_.a ^= fetch8(); set_flags(s_.a, false); break;
    case 0x0D: s_.a = (u8)(s_.a + 1); break;                /* INC A         */
    case 0x0E: s_.a = (u8)(s_.a - 1); break;                /* DEC A         */
    case 0x0F: s_.x = s_.a; break;
    case 0x10: s_.a = s_.x; break;
    case 0x11: s_.x = (u8)(s_.x + 1); break;
    case 0x12: s_.x = (u8)(s_.x - 1); break;
    case 0x13: s_.pc = fetch16(); break;                    /* JMP           */
    case 0x14: { u16 t = fetch16(); if (s_.flags & kZ) s_.pc = t; break; }
    case 0x15: { u16 t = fetch16(); if (!(s_.flags & kZ)) s_.pc = t; break; }
    case 0x16: { u16 t = fetch16(); if (s_.flags & kC) s_.pc = t; break; }
    case 0x17: { u16 t = fetch16(); if (s_.flags & kN) s_.pc = t; break; }
    case 0x18: { u16 t = fetch16(); push((u8)(s_.pc & 0xFF)); push((u8)(s_.pc >> 8)); s_.pc = t; break; }
    case 0x19: { u8 hi = pop(), lo = pop(); s_.pc = (u16)(lo | ((u16)hi << 8)); break; }
    case 0x1A: push(s_.a); break;
    case 0x1B: s_.a = pop(); break;
    case 0x1C: { u16 a = fetch16(); bus_->write_byte(a, s_.a); break; }
    case 0x1D: { u16 a = fetch16(); bus_->read_byte(a, &s_.a); break; }
    case 0x1E: case 0x1F: s_.halted = true; break;          /* BRK / HALT    */
    default:
      /* Undefined: this ISA has no exception model, so it stops.  A real
       * module would raise its own (see the ARM one, which takes an UNDEF). */
      s_.halted = true;
      break;
  }
  ++s_.insns;
  ++s_.cycles;
  stats_.insns_interp = s_.insns;
  stats_.cycles_interp = s_.cycles;
  if (trace_) {
    std::fprintf(stderr, "[toy8] %04x  %02x  a=%02x x=%02x sp=%02x f=%u\n",
                 at, op, s_.a, s_.x, s_.sp, s_.flags);
  }
}

void Core::step() { step_one(); }

/* Both bounds at once, like every module: an instruction that would cross
 * either line is not started.  A halted toy-8 has no interrupt input at all, so
 * it ends the loop instead of charging cycles into a wall. */
u64 Core::run_until(u64 max_insns, u64 max_cycles) {
  const u64 insn_before = s_.insns, cyc_before = s_.cycles;
  stop_ = false;                 /* a stop request belongs to one run       */
  while (!s_.halted && !stop_) {
    if (max_insns && s_.insns - insn_before >= max_insns) break;
    if (max_cycles && s_.cycles - cyc_before >= max_cycles) break;
    step_one();
  }
  return s_.insns - insn_before;
}

u32 Core::get_reg(u32 index) const {
  switch (index) {
    case kRegA: return s_.a;
    case kRegX: return s_.x;
    case kRegSP: return s_.sp;
    case kRegPC: return s_.pc;
    case kRegFlags: return s_.flags;
    default: return 0;
  }
}

bool Core::set_reg(u32 index, u32 value) {
  switch (index) {
    case kRegA: s_.a = (u8)value; return true;
    case kRegX: s_.x = (u8)value; return true;
    case kRegSP: s_.sp = (u8)value; return true;
    case kRegPC: s_.pc = (u16)value; return true;
    case kRegFlags: s_.flags = (u8)value; return true;
    default: return false;
  }
}

void Core::set_pc(u32 addr, bool alternate_state) {
  (void)alternate_state;
  s_.pc = (u16)addr;
}

/* No MMU here: an access as the guest would make it is an access through the
 * bus, which is what a device hook and the JIT's slow path both see. */
u32 Core::guest_read(u32 addr, u32 size) {
  u32 v = 0;
  for (u32 i = 0; i < size; ++i) {
    u8 b = 0;
    bus_->read_byte((u16)(addr + i), &b);
    v |= (u32)b << (8 * i);
  }
  return v;
}

void Core::guest_write(u32 addr, u32 value, u32 size) {
  for (u32 i = 0; i < size; ++i)
    bus_->write_byte((u16)(addr + i), (u8)(value >> (8 * i)));
}

void Core::set_entry(u32 addr, bool alternate_state, u32 stack) {
  (void)alternate_state;
  s_.pc = (u16)addr;
  if (stack) s_.sp = (u8)(stack & 0xFF);
  s_.halted = false;
  stop_ = false;
}

bool Core::set_option(const char* key, const char* value) {
  if (!key) return false;
  if (!std::strcmp(key, "trace")) {
    trace_ = !value || !*value || std::strcmp(value, "0") != 0;
    return true;
  }
  return false;
}

class Toy8Module final : public CpuModule {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo kMod = {kId, "toy-8", ModuleKind::Cpu,
                                    "a five-register demo CPU: the worked "
                                    "example of a CPU module"};
    return kMod;
  }
  std::unique_ptr<CpuCore> create() const override {
    return std::unique_ptr<CpuCore>(new Core());
  }
};

}  // namespace

const CpuModule& module() {
  static const Toy8Module kModule;
  return kModule;
}

void register_module() { brain::ModuleRegistry::instance().add_cpu(&module()); }

}  // namespace brain::toy8

namespace brain {
void register_toy8_module() { toy8::register_module(); }
}  // namespace brain
