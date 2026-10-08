/* brain-emu -- the monitor modules this build ships.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The host side of observing a machine: a `stats` monitor for the CLI's
 * `--monitor` slot (`StatsMonitor`, an observer that counts slices and
 * reports them as fields), and the command monitor a user types into
 * (`CommandMonitor`, a debugger's console over the C API).  Both live in
 * `brain::host` and neither knows any CPU.
 */
#ifndef BRAINEMU_HOST_MONITOR_H
#define BRAINEMU_HOST_MONITOR_H

namespace brain {

/* Adds the stats monitor module to the registry.  Called once by the build's
 * module list (`builtin/builtin.cpp`) - the only place that decides what a
 * build contains. */
void register_monitor_module();

}  // namespace brain

#endif  /* BRAINEMU_HOST_MONITOR_H */
