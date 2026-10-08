/* brain-emu -- devices: the non-CPU modules.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A device is a region of the guest's physical address space plus the behaviour
 * behind it.  The bus does the routing (4 KiB pages, one bound check), so a
 * device never sees an address it was not mapped for and never has to know how
 * big a page is; it gets an offset within its own window.
 *
 * This is deliberately not a register-array abstraction: real peripherals have
 * side effects on read, write-only registers, FIFOs and interrupt lines, and an
 * interface that models "addresses and values" is the only one that does not
 * have to be extended per chip.  A device that wants to be clocked overrides
 * `tick`, which the machine calls after each run slice with the cycles that
 * elapsed (a timer, a DMA engine, a display refresh).
 */
#ifndef BRAINEMU_DEVICE_DEVICE_H
#define BRAINEMU_DEVICE_DEVICE_H

#include <cstdio>

#include "brainemu/module.h"
#include "brainemu/types.h"

namespace brain {

class Bus;
class Machine;

class Device {
 public:
  virtual ~Device() = default;

  virtual const ModuleInfo& info() const = 0;

  /* The window this device answers for.  `attach` maps the covering pages and
   * fails if anything already claims it. */
  virtual u32 base() const = 0;
  virtual u32 size() const = 0;

  virtual void reset() {}
  virtual u32 read(u32 offset, u32 size) = 0;
  virtual void write(u32 offset, u32 value, u32 size) = 0;
  virtual void tick(u64 cycles) { (void)cycles; }

  /* Interrupt outputs - a device's "pins".  A device declares the lines it can
   * drive (`irq_count`/`irq_name`) and raises or lowers them with `drive_irq`;
   * the *board* decides where each line goes (MachineConfig::wires, or
   * `--wire DEV.OUT=INPUT`), so neither end has to know the other's type.  The
   * machine reports every change to whoever is wired - and drops it when nothing
   * is, which is what lets the same device run in a machine that ignores its
   * interrupts.  Levels are what a line carries: a device that wants an edge
   * drives high and then low (see `pulse_irq`). */
  virtual u32 irq_count() const { return 0; }
  virtual const char* irq_name(u32 i) const { (void)i; return ""; }

  static constexpr u32 kMaxIrq = 8;
  void drive_irq(u32 i, int level);
  void pulse_irq(u32 i) { drive_irq(i, 1); drive_irq(i, 0); }
  int irq_level(u32 i) const { return i < kMaxIrq ? irq_level_[i] : 0; }

  /* Interrupt *inputs* - the other half of a pin, and what lets devices be
   * chained (two timers sharing one interrupt controller, a controller feeding a
   * CPU).  A device that has them declares the names a wire may target
   * (`input_count`/`input_name`) and is told every level change with
   * `set_input`; the default has no inputs, so a peripheral that only speaks
   * outwards needs no code for this.  A device that combines its inputs simply
   * drives its own output from here, and the machine carries that on to whatever
   * the next wire names - an interrupt controller is nothing more than that
   * (see devices/interrupt/intc/). */
  virtual u32 input_count() const { return 0; }
  virtual const char* input_name(u32 i) const { (void)i; return ""; }
  virtual void set_input(u32 i, int level) { (void)i; (void)level; }

  /* The name this *instance* answers to on its machine, which is how a wire (or
   * a tool) tells two devices built from the same module apart: `device_by_id`
   * matches it first, and only falls back to the module id.  The machine sets it
   * when it builds the device; on its own the instance is the module's id, so a
   * device used outside a machine still has a name. */
  const char* instance() const { return instance_ ? instance_ : info().id; }
  void set_instance(const char* name) { instance_ = name; }

  /* Where the machine installs itself: a device never needs the machine's type,
   * and a device used outside a machine (a unit test, a GUI) simply has no
   * notifier and drives its levels into the void. */
  using IrqNotify = void (*)(void* user, Device& d, u32 output, int level);
  void set_irq_notify(IrqNotify fn, void* user) { irq_fn_ = fn; irq_user_ = user; }

  /* Called once, by `Machine::add_device`, before `attach`: a device that has
   * to *do* something to the machine around it - stop the run loop, raise an
   * interrupt line, read the CPU's counters - keeps the pointer.  A plain
   * memory-mapped peripheral ignores it, and so does a device the caller owns
   * and talks to directly. */
  virtual void on_attach(Machine& m) { (void)m; }

  /* Self-description, for tools: any name/value pair this device wants a
   * `--stats` print, a debugger or a GUI to show (an exit code, a byte count, a
   * log, a FIFO level).  The core never interprets a field - it just asks - so a
   * new peripheral surfaces its state without a core change.  Names are short
   * and stable ("code", "hits", "log"), values are text. */
  virtual u32 field_count() const { return 0; }
  virtual const char* field_name(u32 i) const { (void)i; return ""; }
  virtual const char* field_value(u32 i) const { (void)i; return ""; }

  /* Map into `bus` and reset the device.  The bus publishes regions a page at a
   * time, so a window smaller than a page (a UART's four bytes, say) maps the
   * whole covering page; the device's own `size()` is unaffected, and offsets
   * are always relative to `base()`.  Returns false when the window overlaps an
   * existing region or is empty.  A device whose memory the bus maps directly -
   * see BufferDevice - overrides this. */
  virtual bool attach(Bus& bus);
  bool attached() const { return attached_; }
  const char* name() const { return info().name; }

  /* The page range a window covering [base, base+size) needs. */
  static u32 covering_size(u32 size) { return (size + 0xFFFu) & ~0xFFFu; }
  void mark_attached() { attached_ = true; }

 private:
  bool attached_ = false;
  const char* instance_ = nullptr;
  int irq_level_[kMaxIrq] = {};
  IrqNotify irq_fn_ = nullptr;
  void* irq_user_ = nullptr;
};

/* Adapters for the two shapes a caller usually already has: free functions with
 * a `user` pointer (what the C API's callbacks are) and a plain buffer.  Both
 * are devices, so `Machine::add_device` takes them the same way it takes a
 * hand-written peripheral. */
class FuncDevice final : public Device {
 public:
  using ReadFn = u32 (*)(void* user, u32 addr, u32 size);
  using WriteFn = void (*)(void* user, u32 addr, u32 value, u32 size);

  FuncDevice(const char* id, const char* name, u32 base, u32 size, ReadFn rd,
             WriteFn wr, void* user);

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  u32 size() const override { return size_; }
  u32 read(u32 offset, u32 size) override;
  void write(u32 offset, u32 value, u32 size) override;

 private:
  ModuleInfo info_{};
  u32 base_ = 0, size_ = 0;
  ReadFn rd_ = nullptr;
  WriteFn wr_ = nullptr;
  void* user_ = nullptr;
};

/* A block of RAM or ROM the caller owns: `attach` maps the caller's buffer, so a
 * GUI or a test can share memory with the machine instead of copying in and out
 * of it.  Read-only unless `writable`. */
class BufferDevice final : public Device {
 public:
  BufferDevice(const char* id, const char* name, u32 base, u32 size, u8* host,
               bool writable);

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  u32 size() const override { return size_; }
  /* The bus serves these pages straight from `host`, so no callback is ever
   * reached: the member functions exist to satisfy the interface (and to be
   * honest about what a directly mapped device does). */
  u32 read(u32 offset, u32 size) override;
  void write(u32 offset, u32 value, u32 size) override;
  bool attach(Bus& bus) override;

 private:
  ModuleInfo info_{};
  u32 base_ = 0, size_ = 0;
  u8* host_ = nullptr;
  bool writable_ = false;
};

}  // namespace brain

#endif  /* BRAINEMU_DEVICE_DEVICE_H */
