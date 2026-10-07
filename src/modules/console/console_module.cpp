/* brain-emu -- the test console, as a device module.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Every byte written to this window is appended to a line buffer the host can
 * print, which is all a bare-metal image with no UART driver needs.  The class
 * is `ConsoleDevice` (include/brainemu/device/device.h); this file is only its
 * module wrapper, and together they are the example of a device module that is
 * *pure* state - it needs no machine, no clock and no bus pointer, so
 * `on_attach` never matters to it.  Being a module means the same console can be
 * selected by id (`--device console`), listed by `--list-modules`, and described
 * to a tool through its fields, none of which the core had to learn about.
 */
#include "brainemu/core/machine.h"
#include "brainemu/core/module.h"
#include "brainemu/device/device.h"

namespace brain {

namespace {

/* The window the images in this tree use by default. */
constexpr u32 kDefaultBase = 0x10000000u;

class ConsoleModule final : public DeviceModule {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo mi = {
        "console", "test console", ModuleKind::Device,
        "bytes written here are collected as text for the host to print"};
    return mi;
  }

  std::unique_ptr<Device> create(u32 base) const override {
    return std::unique_ptr<Device>(new ConsoleDevice(base));
  }

  u32 default_base() const override { return kDefaultBase; }
};

}  // namespace

void register_console_module() {
  static const ConsoleModule m;
  ModuleRegistry::instance().add_device(&m);
}

}  // namespace brain
