/* brain-emu -- the exit port, as a device module.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A bare-metal test image has to say "I am done" somewhere.  The conventional
 * answer on emulated ARM boards is a write-only word: the image stores a value
 * into it and the emulator stops, that value readable afterwards as the result.
 * That is a *peripheral*, so it is a device module like any other - and writing
 * it this way is the point: a caller selects it by id (`--device exit`), the core
 * never hears about it, and a board that already has something at that address
 * simply does not select it.
 *
 * It is also the shortest example of a device that acts on the *machine* rather
 * than on memory: the write stops the run loop (`Machine::stop`), which is the
 * one thing a device cannot do alone, and it does it through the pointer
 * `on_attach` handed it.
 */
#include "devices/system/exit/exit.h"

#include "brainemu/machine.h"
#include "brainemu/module.h"
#include "brainemu/device.h"

#include <cstdio>

namespace brain {

namespace {

/* The i.MX28 "test" port range (0x10000010-0x1000001F) is where a bare-metal
 * image conventionally puts this; it is the module's *default*, not a rule - the
 * window moves with the base a caller passes. */
constexpr u32 kDefaultBase = 0x10000010u;

class ExitDevice final : public Device {
 public:
  explicit ExitDevice(u32 base) : base_(base) {
    info_.id = "exit";
    info_.name = "exit port";
    info_.kind = ModuleKind::Device;
    info_.description =
        "a store here stops the machine; the value is kept as the exit code";
  }

  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return base_; }
  u32 size() const override { return 4; }

  void on_attach(Machine& m) override { machine_ = &m; }

  void reset() override {
    code_ = 0;
    hits_ = 0;
  }

  u32 read(u32 offset, u32 size) override {
    (void)offset;
    (void)size;
    return code_;
  }

  void write(u32 offset, u32 value, u32 size) override {
    (void)size;
    if (offset != 0) return;
    code_ = value;
    ++hits_;
    if (machine_) machine_->stop();
  }

  /* What a harness reads back: the value, the byte the C convention would exit
   * with, and how many times the image wrote one. */
  u32 field_count() const override { return 3; }
  const char* field_name(u32 i) const override {
    static const char* n[] = {"code", "status", "writes"};
    return i < 3 ? n[i] : "";
  }
  const char* field_value(u32 i) const override {
    static char buf[3][16];
    if (i == 0) std::snprintf(buf[0], sizeof(buf[0]), "0x%08x", code_);
    else if (i == 1) std::snprintf(buf[1], sizeof(buf[1]), "%u", code_ & 0xFFu);
    else if (i == 2) std::snprintf(buf[2], sizeof(buf[2]), "%u", hits_);
    else return "";
    return buf[i];
  }

  u32 code() const { return code_; }
  u32 hits() const { return hits_; }

 private:
  ModuleInfo info_{};
  u32 base_ = kDefaultBase;
  u32 code_ = 0;
  u32 hits_ = 0;
  Machine* machine_ = nullptr;
};

class ExitModule final : public DeviceModule {
 public:
  const ModuleInfo& info() const override {
    static const ModuleInfo mi = {
        "exit", "exit port", ModuleKind::Device,
        "a store here stops the machine; the value is kept as the exit code"};
    return mi;
  }

  std::unique_ptr<Device> create(u32 base) const override {
    return std::unique_ptr<Device>(new ExitDevice(base));
  }

  u32 default_base() const override { return kDefaultBase; }
};

}  // namespace

void register_exit_module() {
  static const ExitModule m;
  ModuleRegistry::instance().add_device(&m);
}

}  // namespace brain
