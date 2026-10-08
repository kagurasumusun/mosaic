/* brain-emu -- the console device.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A window that turns writes into text: the smallest thing a bare-metal image
 * needs in order to say something, and the reference for "a device is a window
 * plus behaviour".  A read finds nothing waiting (the log is drained from the
 * host side), which is what a UART's data register looks like when it is empty.
 *
 * This is the device, not the module: `console.cpp` wraps it for the registry,
 * and nothing in the core knows this class exists - the core only ever sees
 * `Device`.  A build without a console is then a build without a console, not a
 * core with a hole in it.
 */
#ifndef BRAINEMU_DEVICES_CONSOLE_DEVICE_H
#define BRAINEMU_DEVICES_CONSOLE_DEVICE_H

#include <cstdio>

#include "brainemu/device.h"
#include "brainemu/types.h"

namespace brain {

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

#endif  /* BRAINEMU_DEVICES_CONSOLE_DEVICE_H */
