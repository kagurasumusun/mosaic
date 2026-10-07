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

#include "brainemu/core/module.h"
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

/* A test-bench console: every byte written to the window is appended to a log
 * the host can print.  Handy for bare-metal images with no UART driver.  It is
 * a normal device, so it attaches like any other. */
class ConsoleDevice final : public Device {
 public:
  explicit ConsoleDevice(u32 base = 0x10000000u);

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  /* One data register; the bus maps the page it sits in. */
  u32 size() const override { return 4; }

  u32 read(u32 offset, u32 size) override;
  void write(u32 offset, u32 value, u32 size) override;

  const char* log() const { return log_; }
  u32 log_len() const { return len_; }
  void clear() { len_ = 0; log_[0] = 0; }

  /* What the host wanted to see: the text so far, and how much of it there is
   * (`len` in the field list rather than the log itself, because a field value
   * is one line and a log is not). */
  u32 field_count() const override { return 2; }
  const char* field_name(u32 i) const override {
    return i == 0 ? "bytes" : "log";
  }
  const char* field_value(u32 i) const override {
    static char n[16];
    if (i == 0) {
      std::snprintf(n, sizeof(n), "%u", len_);
      return n;
    }
    return i == 1 ? (len_ ? log_ : "(empty)") : "";
  }

  static constexpr u32 kLogSize = 4096;

 private:
  ModuleInfo info_{};
  u32 base_ = 0;
  u32 len_ = 0;
  char log_[kLogSize] = {};
};

}  // namespace brain

#endif  /* BRAINEMU_DEVICE_DEVICE_H */
