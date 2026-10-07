/* The modules this build contains.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * A module registers itself, and this is the one place that says which modules
 * exist.  It has to be a function rather than a static initialiser because the
 * library is a static archive: an object file nothing references is not linked
 * in, so a module that only had a static registration object would silently
 * vanish from the build.  A build that drops a module drops a line here, and
 * gets a linker error that names the module instead of a runtime "no CPU
 * modules" surprise.
 *
 * Adding a processor is therefore:
 *   1. src/modules/<id>/ with its CpuCore implementation and its module object,
 *   2. one `register_<id>_module()` declaration and one call below,
 *   3. the directory in the Makefile's module list (it globs, so usually
 *      nothing at all).
 *
 * The same three steps add a *device* or a *monitor* module; the only difference
 * is which `add_*` the module calls on the registry and which factory it
 * implements (DeviceModule / MonitorModule).  The two the core used to build
 * itself - the exit port and the console - are here for exactly that reason.
 */
#include "brainemu/core/module.h"

#include "brainemu/loader/loader.h"

namespace brain {

/* Defined in each module's translation unit, so the linker keeps them. */
void register_arm926_module();
void register_toy8_module();
void register_exit_module();
void register_console_module();

void register_builtin_modules() {
  static bool done = false;
  if (done) return;
  done = true;
  register_arm926_module();     /* the primary target: ARM926EJ-S (ARMv5TEJ) */
  register_toy8_module();       /* the worked example of a CPU module        */
  register_builtin_loaders();   /* elf32 + raw                               */
  register_exit_module();       /* devices: the two adapters this build ships */
  register_console_module();
}

}  // namespace brain
