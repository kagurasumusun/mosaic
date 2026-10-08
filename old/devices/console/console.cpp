/* brain-emu -- the console module.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Every byte written to this device's window is appended to a line buffer the
 * host can print, which is all a bare-metal image with no UART driver needs.
 * The device itself is `ConsoleDevice` (`console_device.h`); this file is its
 * module wrapper, and the two together are the example of a device that is a
 * window plus a side effect.
 */
#include "devices/console/console.h"

#include <cstring>
#include <memory>

#include "brainemu/bus.h"
#include "brainemu/module.h"
#include "devices/console/console_device.h"

namespace brain {

ConsoleDevice::ConsoleDevice(u32 base) : base_(base) {
  info_.id = "console";
  info_.name = "test console";
  info_.kind = ModuleKind::Device;
  info_.description = "bytes written to the window, collected as text";
  log_[0] = 0;
}

u32 ConsoleDevice::read(u32 offset, u32 size) {
  (void)offset; (void)size;
  /* A UART's data register would be here.  The log is drained from the host
   * side (`log()`), so a guest read finds nothing waiting. */
  return 0;
}

void ConsoleDevice::write(u32 offset, u32 value, u32 size) {
  (void)offset;
  for (u32 i = 0; i < size; ++i) {
    if (len_ + 1 >= kLogSize) {
      /* Keep the tail: a guest that prints a lot should still show the end. */
      std::memmove(log_, log_ + kLogSize / 2, kLogSize / 2);
      len_ = kLogSize / 2;
    }
    log_[len_++] = (char)(value >> (8 * i));
  }
  log_[len_] = 0;
}

namespace {

class ConsoleModule final : public DeviceModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  std::unique_ptr<Device> create(u32 base) const override {
    return std::unique_ptr<Device>(new ConsoleDevice(base));
  }
  u32 default_base() const override { return 0x10000000u; }

 private:
  ModuleInfo info_{"console", "test console", ModuleKind::Device,
                   "bytes written here are collected as text for the host to print"};
};

}  // namespace

void register_console_module() {
  static const ConsoleModule m;
  ModuleRegistry::instance().add_device(&m);
}

}  // namespace brain
