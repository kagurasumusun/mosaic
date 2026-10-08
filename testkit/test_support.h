/* brain-emu -- what every test suite shares.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A suite is one binary per component (`core/tests/test_machine.cpp`,
 * `devices/tests/test_devices.cpp`, ...), which keeps a component's tests next
 * to the component and makes `make check` list them by name.  What they share is
 * only this: a counter, a `CHECK`, and the three little helpers that turn a
 * device's registers into something a test can assert on.
 *
 * Nothing here knows about any machine, device or loader; a suite that needs
 * more (an image builder, a guest) keeps it in its own file, where the fixture it
 * belongs to is.
 */
#ifndef BRAINEMU_TESTKIT_TEST_SUPPORT_H
#define BRAINEMU_TESTKIT_TEST_SUPPORT_H

#include <cstdio>
#include <cstring>

#include "brainemu/bus.h"
#include "brainemu/device.h"
#include "brainemu/module.h"
#include "brainemu/module.h"
#include "brainemu/types.h"
#include "builtin/builtin.h"

namespace brainemutest {

inline int g_checks = 0;
inline int g_failed = 0;

inline void check(bool ok, const char* what) {
  ++g_checks;
  if (!ok) {
    ++g_failed;
    std::printf("  FAILED: %s\n", what);
  }
}

/* The registry is filled by the build's module list, and the core calls it when it
 * builds a machine.  A suite that inspects the registry *directly* (what does this
 * build contain?) has to ask for it first: the order is the behaviour under test,
 * so it is a named call rather than something hidden in a constructor. */
inline void ensure_builtins() { brain::register_builtin_modules(); }

/* The suite's verdict, and the process's exit code. */
inline int summary(const char* suite) {
  if (g_failed) {
    std::printf("FAIL: %d/%d checks passed (%s)\n", g_checks - g_failed, g_checks,
                suite);
    return 1;
  }
  std::printf("PASS: %d/%d checks passed (%s)\n", g_checks, g_checks, suite);
  return 0;
}

}  // namespace brainemutest

#define CHECK(cond) ::brainemutest::check((cond), #cond)

/* `check(cond, "what it means")` - for the checks where the expression is not the
 * story ("the guest stopped the run itself"). */
using brainemutest::check;

using namespace brain;

namespace {

/* A device that records what the bus did to it - the sharpest way to state what
 * the machine promises a *device*: attach resets it, a tick tells it time passed,
 * a write arrives with its size, and a reset is not a reason to lose what came
 * before.  Two suites drive it (the core's and toy8's), so it lives here; the
 * monitor pair below is the same idea for the observation side. */
/* A device that records what it saw, with a tick counter and a reset hook. */
struct Recorder {
  u32 writes[8] = {};
  u32 nwrites = 0;
  u32 last_size = 0;
  u64 ticks = 0;
  u32 resets = 0;
};

class RecorderDevice final : public Device {
 public:
  explicit RecorderDevice(Recorder* r) : r_(r) {
    info_.id = "recorder";
    info_.name = "recorder";
    info_.kind = ModuleKind::Device;
  }
  const ModuleInfo& info() const override { return info_; }
  u32 base() const override { return 0xF000u; }   /* reachable in 16 bits */
  u32 size() const override { return 0x1000u; }
  u32 read(u32, u32) override { return 0xDEADBEEFu; }
  void write(u32, u32 value, u32 size) override {
    if (r_->nwrites < 8) r_->writes[r_->nwrites++] = value;
    r_->last_size = size;
  }
  void tick(u64 cycles) override { r_->ticks += cycles; }
  void reset() override { ++r_->resets; }

 private:
  Recorder* r_;
  ModuleInfo info_;
};

/* A device's registers, read and written the way a guest would: through the bus,
 * so the test goes through the same window a program would. */
[[maybe_unused]] u32 reg32(Bus& bus, u32 addr) {
  u32 v = 0;
  for (u32 i = 0; i < 4; ++i) {
    u8 b = 0;
    bus.read_byte(addr + i, &b);
    v |= (u32)b << (8 * i);
  }
  return v;
}

[[maybe_unused]] void put32(Bus& bus, u32 addr, u32 value) {
  for (u32 i = 0; i < 4; ++i) bus.write_byte(addr + i, (u8)(value >> (8 * i)));
}

/* What an interrupt line did: the levels it was driven to, in order.  A device
 * drives its output; the machine tells whoever is wired to it; this records what
 * arrived, which is how a test asserts on *deliveries* rather than on state. */
struct LevelLog {
  int calls = 0;
  int levels[16] = {};
  u32 outputs[16] = {};
};

[[maybe_unused]] void log_irq(void* user, Device&, u32 output, int level) {
  LevelLog* l = static_cast<LevelLog*>(user);
  if (l->calls < 16) {
    l->levels[l->calls] = level;
    l->outputs[l->calls] = output;
  }
  ++l->calls;
}

}  // namespace

#endif  /* BRAINEMU_TESTKIT_TEST_SUPPORT_H */
