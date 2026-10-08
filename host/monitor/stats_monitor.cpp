/* brain-emu -- the stats monitor: the tallies as fields, and the module wrapper
 * that puts the monitor in the registry.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#include "host/monitor/stats_monitor.h"

#include <cstdio>
#include <memory>

#include "brainemu/module.h"

namespace brain::host {

const char* StatsMonitor::field_value(u32 i) const {
  static char buf[32];
  switch (i) {
    case 0: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)slices_); break;
    case 1: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)retired_); break;
    case 2: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)last_slice_); break;
    case 3: std::snprintf(buf, sizeof(buf), "%llu", (unsigned long long)resets_); break;
    default: return "";
  }
  return buf;
}

namespace {

class StatsMonitorModule final : public MonitorModule {
 public:
  const ModuleInfo& info() const override { return info_; }
  std::unique_ptr<Monitor> create() const override {
    return std::unique_ptr<Monitor>(new StatsMonitor());
  }

 private:
  ModuleInfo info_{"stats", "run-loop tallies", ModuleKind::Monitor,
                   "counts slices, retired instructions and resets"};
};

}  // namespace

}  // namespace brain::host

namespace brain {

/* Declared in host/monitor/monitor.h; called once by builtin/builtin.cpp. */
void register_monitor_module() {
  static const host::StatsMonitorModule m;
  ModuleRegistry::instance().add_monitor(&m);
}

}  // namespace brain
