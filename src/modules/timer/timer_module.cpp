/* brain-emu -- a programmable down-counter that can interrupt.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This device is not a model of any particular chip's timer; it is the smallest
 * peripheral that exercises the parts of the *core* a board needs: a window in
 * the address space, a clock (`tick`), a state a tool can print (`field_*`), and
 * an interrupt output that a board wires wherever it likes.  A real SoC timer
 * (the i.MX28's EPIT, say) is this file with the register map of that chip.
 *
 * Registers, all 32-bit, byte-addressable:
 *
 *   0x00  PERIOD   cycles between expiries; writing it restarts the counter
 *   0x04  COUNT    cycles remaining (read-only)
 *   0x08  CTRL     bit 0: enable (read/write; the only bit there is)
 *   0x0C  STATUS   bit 0: pending (latched), bit 1: running (read-only)
 *   0x10  ACK      write anything to clear the pending state and drop the line
 *
 * The acknowledge is its *own* register on purpose.  A bit inside CTRL would
 * make "acknowledge" and "keep running" the same write, and a guest that acks
 * without repeating the enable bit would silently stop its own timer - a trap
 * this file fell into once, with a demo that acked itself to death.
 *
 * The interrupt is a *level*, and the level is what the line carries: it goes
 * high on the first expiry and stays high until the guest acknowledges.  That is
 * the honest model of a level-sensitive input, and it means a guest that misses
 * one expiry still sees the interrupt - and `ticks` counts the expiries, so a
 * test can tell how many the device produced while the CPU was masked.
 */
#include <cstdio>
#include <cstring>

#include "brainemu/core/machine.h"
#include "brainemu/core/module.h"
#include "brainemu/device/device.h"

namespace brain {

class TimerDevice final : public Device {
 public:
  explicit TimerDevice(u32 base) : base_(base) {
    info_.id = "timer";
    info_.name = "programmable timer";
    info_.kind = ModuleKind::Device;
    info_.description = "32-bit down-counter with one interrupt output";
  }

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  u32 size() const override { return 0x14; }   /* through the ACK register */

  void reset() override {
    period_ = 0;
    count_ = 0;
    enable_ = false;
    pending_ = false;
    ticks_ = 0;
    drive_irq(0, 0);
  }

  u32 read(u32 offset, u32 size) override {
    u32 word = 0;
    switch (offset & ~3u) {
      case 0x00: word = period_; break;
      case 0x04: word = count_; break;
      case 0x08: word = (enable_ ? 1u : 0u); break;
      case 0x0C: word = (pending_ ? 1u : 0u) | (enable_ ? 2u : 0u); break;
      case 0x10: word = 0; break;                 /* write-only: reads as zero */
      default: return 0;
    }
    return extract(word, offset, size);
  }

  void write(u32 offset, u32 value, u32 size) override {
    /* A 1- or 2-byte write patches the bytes it covers inside the register, the
     * way a byte-wide driver expects; a 4-byte write replaces it. */
    const u32 cur = read(offset, 4);
    const u32 v = patch(cur, offset, size, value);
    switch (offset & ~3u) {
      case 0x00:
        period_ = v;
        count_ = period_;
        break;
      case 0x08:
        enable_ = (v & 1u) != 0;
        break;
      case 0x10:                             /* acknowledge */
        pending_ = false;
        drive_irq(0, 0);
        break;
      default: break;                        /* COUNT and STATUS are read-only */
    }
  }

  /* The clock.  A timer that has been running for 8 periods by the time anyone
   * looks has produced 8 expiries, not one - `ticks` says so, and the pending
   * bit is a latch, so nothing is lost while the CPU has interrupts masked. */
  void tick(u64 cycles) override {
    if (!enable_ || period_ == 0) return;
    if ((u64)count_ > cycles) {              /* the expiry is still ahead    */
      count_ -= (u32)cycles;
      return;
    }
    /* The counter fires once per period: the first expiry is `count_` cycles
     * away, and whatever clock is left over after it holds `left / period_` more
     * of them.  Counting the passing expiry from the *whole* delta (as an
     * `+ period_` here once did) reports one expiry too many every time the
     * clock crosses one, which a guest sees as an interrupt storm and a test as
     * a wrong `ticks` - the latch hides it, the count does not. */
    const u64 left = cycles - count_;        /* clock left after the expiry  */
    const u64 n = 1 + left / period_;
    count_ = (u32)(period_ - (left % period_));
    ticks_ += n;
    if (!pending_) {
      pending_ = true;
      drive_irq(0, 1);
    }
  }

  /* One interrupt output, and a name a board wires by. */
  u32 irq_count() const override { return 1; }
  const char* irq_name(u32 i) const override { return i == 0 ? "irq" : ""; }

  u32 field_count() const override { return 4; }
  const char* field_name(u32 i) const override {
    static const char* kNames[4] = {"period", "ticks", "pending", "running"};
    return i < 4 ? kNames[i] : "";
  }
  const char* field_value(u32 i) const override {
    static char buf[32];
    switch (i) {
      case 0: std::snprintf(buf, sizeof(buf), "%u", period_); break;
      case 1: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)ticks_); break;
      case 2: std::snprintf(buf, sizeof(buf), "%s", pending_ ? "yes" : "no"); break;
      case 3: std::snprintf(buf, sizeof(buf), "%s", enable_ ? "yes" : "no"); break;
      default: return "";
    }
    return buf;
  }

 private:
  static u32 extract(u32 word, u32 offset, u32 size) {
    const u32 shift = (offset & 3u) * 8u;
    const u32 masked = word >> shift;
    return size >= 4 ? word : (masked & ((1u << (size * 8u)) - 1u));
  }
  static u32 patch(u32 word, u32 offset, u32 size, u32 value) {
    if (size >= 4) return value;
    const u32 shift = (offset & 3u) * 8u;
    const u32 mask = ((1u << (size * 8u)) - 1u) << shift;
    return (word & ~mask) | ((value << shift) & mask);
  }

  ModuleInfo info_{};
  u32 base_ = 0;
  u32 period_ = 0;
  u32 count_ = 0;
  u64 ticks_ = 0;
  bool enable_ = false;
  bool pending_ = false;
};

class TimerModule final : public DeviceModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  std::unique_ptr<Device> create(u32 base) const override {
    return std::unique_ptr<Device>(new TimerDevice(base));
  }
  u32 default_base() const override { return 0x10001000u; }

 private:
  ModuleInfo info_{"timer", "programmable timer", ModuleKind::Device,
                   "32-bit down-counter that can raise one interrupt"};
};

}  // namespace brain

namespace brain {
void register_timer_module() {
  static const TimerModule mod;
  ModuleRegistry::instance().add_device(&mod);
}
}  // namespace brain
