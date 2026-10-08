/* brain-emu -- the stats monitor: a Monitor that counts what the machine did.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The monitor the CLI's `--monitor stats` installs: an observer that is told
 * about every slice the run loop finishes and reports the tallies as fields, so
 * a `--stats` print or a GUI shows "slices, retired, last slice, resets" next to
 * the machine's own counters.  It is a `Monitor` - the core's observer
 * interface - which is the whole point: no core change was needed to add a
 * counter a user can watch.
 */
#ifndef BRAINEMU_HOST_MONITOR_STATS_MONITOR_H
#define BRAINEMU_HOST_MONITOR_STATS_MONITOR_H

#include "brainemu/monitor.h"
#include "brainemu/types.h"

namespace brain::host {

class StatsMonitor final : public Monitor {
 public:
  const ModuleInfo& info() const override { return info_; }

  void on_slice(Machine& m, u64 retired) override {
    (void)m;
    ++slices_;
    retired_ += retired;
    last_slice_ = retired;
  }
  void on_reset(Machine& m) override {
    (void)m;
    ++resets_;
  }

  u32 field_count() const override { return 4; }
  const char* field_name(u32 i) const override {
    static const char* kNames[4] = {"slices", "retired", "last_slice", "resets"};
    return i < 4 ? kNames[i] : "";
  }
  const char* field_value(u32 i) const override;

 private:
  ModuleInfo info_{"stats", "run-loop tallies", ModuleKind::Monitor,
                   "counts slices, retired instructions and resets"};
  u64 slices_ = 0;
  u64 retired_ = 0;
  u64 last_slice_ = 0;
  u64 resets_ = 0;
};

}  // namespace brain::host

#endif  /* BRAINEMU_HOST_MONITOR_STATS_MONITOR_H */
