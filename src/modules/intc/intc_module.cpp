/* brain-emu -- an interrupt controller: several inputs, one output.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This is the other half of what a device can be, and the reason `Device` has
 * input pins at all.  A peripheral speaks outwards - it drives an interrupt line
 * and knows nothing else; a controller is the opposite: it *listens* to several
 * lines, applies a mask, and drives one.  Neither needs to know what the other
 * is, and the CPU needs to know about neither, because a wire is just two names
 * (see MachineConfig::wires): the board says
 *
 *   t0.irq -> intc.irq0,  t1.irq -> intc.irq1,  intc.irq -> irq
 *
 * and a machine with two timers and one CPU interrupt line works without a line
 * of code in any of the three modules.  A real SoC's interrupt collector (the
 * i.MX28's, or an ARM PL190) is this file with that chip's register map, more
 * inputs and per-line priorities.
 *
 * Registers, all 32-bit, byte-addressable:
 *
 *   0x00  MASK     bit i: line i is enabled (reset: 0 - nothing is enabled
 *                  until software says so, which is what an interrupt
 *                  controller's reset state is)
 *   0x04  STATUS   bit i: line i is asserted right now (read-only: the input
 *                  pins plus any software interrupt)
 *   0x08  PENDING  (lines | software) & MASK, read-only - what the output is the
 *                  OR of
 *   0x0C  SET      write 1 to a bit to raise a software interrupt on that line
 *                  (so a guest or a test can interrupt itself without a
 *                  peripheral)
 *   0x10  CLEAR    write 1 to a bit to drop it again
 *
 * The output is a level and follows PENDING exactly: it goes high when the first
 * unmasked line asserts and low when the last one lets go.  There is no tick -
 * a controller is combinational, and a device that needs no clock simply does
 * not override one.
 */
#include <cstdio>
#include <memory>

#include "brainemu/core/machine.h"
#include "brainemu/core/module.h"
#include "brainemu/device/device.h"

namespace brain {

class IntcDevice final : public Device {
 public:
  explicit IntcDevice(u32 base) : base_(base) {
    info_.id = "intc";
    info_.name = "interrupt controller";
    info_.kind = ModuleKind::Device;
    info_.description = "4 interrupt inputs, a mask and one interrupt output";
  }

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  u32 size() const override { return 0x14; }

  void reset() override {
    mask_ = 0;
    lines_ = 0;
    soft_ = 0;
    drive_irq(0, 0);
  }

  u32 read(u32 offset, u32 size) override {
    u32 word = 0;
    switch (offset & ~3u) {
      case 0x00: word = mask_; break;
      case 0x04: word = lines_ | soft_; break;
      case 0x08: word = pending(); break;
      default: return 0;                 /* SET/CLEAR are write-only */
    }
    return extract(word, offset, size);
  }

  void write(u32 offset, u32 v, u32 size) override {
    if (size < 4) v = merge(offset, size, v);
    switch (offset & ~3u) {
      case 0x00: mask_ = v & kLineMask; break;
      case 0x04: break;                  /* STATUS is read-only            */
      case 0x08: break;                  /* PENDING is read-only           */
      case 0x0C: soft_ |= v & kLineMask; break;
      case 0x10: soft_ &= ~(v & kLineMask); break;
      default: return;
    }
    update();
  }

  /* The input pins: what a wire targets.  Only a level *change* matters - a
   * device that recomputed on every delivery would be doing arithmetic on
   * nothing - and a device is free to be wired to nothing at all. */
  u32 input_count() const override { return kLines; }
  const char* input_name(u32 i) const override {
    static const char* kNames[kLines] = {"irq0", "irq1", "irq2", "irq3"};
    return i < kLines ? kNames[i] : "";
  }
  void set_input(u32 i, int level) override {
    if (i >= kLines) return;
    const u32 bit = 1u << i;
    const u32 before = lines_;
    if (level) lines_ |= bit;
    else lines_ &= ~bit;
    if (lines_ != before) update();
  }

  /* One output, and the name a board wires by. */
  u32 irq_count() const override { return 1; }
  const char* irq_name(u32 i) const override { return i == 0 ? "irq" : ""; }

  u32 field_count() const override { return 3; }
  const char* field_name(u32 i) const override {
    static const char* kNames[3] = {"lines", "mask", "pending"};
    return i < 3 ? kNames[i] : "";
  }
  const char* field_value(u32 i) const override {
    static char buf[16];
    switch (i) {
      case 0: std::snprintf(buf, sizeof(buf), "0x%x", lines_ | soft_); break;
      case 1: std::snprintf(buf, sizeof(buf), "0x%x", mask_); break;
      case 2: std::snprintf(buf, sizeof(buf), "0x%x", pending()); break;
      default: return "";
    }
    return buf;
  }

 private:
  static constexpr u32 kLines = 4;
  static constexpr u32 kLineMask = (1u << kLines) - 1u;

  u32 pending() const { return (lines_ | soft_) & mask_; }
  void update() { drive_irq(0, pending() ? 1 : 0); }

  /* The register's whole word, without the read path's narrowing (a byte-wide
   * write has to see the bytes it is not touching). */
  u32 reg_word(u32 offset) const {
    switch (offset & ~3u) {
      case 0x00: return mask_;
      case 0x04: return lines_ | soft_;
      case 0x08: return pending();
      default: return 0;
    }
  }

  static u32 extract(u32 word, u32 offset, u32 size) {
    const u32 shift = (offset & 3u) * 8u;
    const u32 masked = word >> shift;
    return size >= 4 ? word : (masked & ((1u << (size * 8u)) - 1u));
  }
  /* A narrow write touches only its bytes: the value is shifted into place with
   * the neighbouring bytes taken from the register itself, so a byte-wide poke
   * at the mask does not clear the other lines. */
  u32 merge(u32 offset, u32 size, u32 v) const {
    const u32 shift = (offset & 3u) * 8u;
    const u32 bits = size * 8u;
    const u32 word = reg_word(offset);
    const u32 mask = ((1u << bits) - 1u) << shift;
    return (word & ~mask) | ((v << shift) & mask);
  }

  u32 base_;
  ModuleInfo info_;
  u32 mask_ = 0;
  u32 lines_ = 0;
  u32 soft_ = 0;
};

class IntcDeviceModule final : public DeviceModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  std::unique_ptr<Device> create(u32 base) const override {
    return std::unique_ptr<Device>(new IntcDevice(base));
  }
  u32 default_base() const override { return 0x10002000u; }

 private:
  ModuleInfo info_ = [] {
    ModuleInfo i;
    i.id = "intc";
    i.name = "interrupt controller";
    i.kind = ModuleKind::Device;
    i.description = "4 interrupt inputs, a mask, and one interrupt output";
    return i;
  }();
};

void register_intc_module() {
  static const IntcDeviceModule mod;
  ModuleRegistry::instance().add_device(&mod);
}

}  // namespace brain
