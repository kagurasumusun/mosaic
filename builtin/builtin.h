/* The modules this build contains.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The one place that says which machines, devices, loaders and boards exist.
 * See `builtin.cpp`.
 */
#ifndef BRAINEMU_BUILTIN_BUILTIN_H
#define BRAINEMU_BUILTIN_BUILTIN_H

namespace brain {

/* Adds this module to the registry.  Called once by the build's module list
 * (`builtin/builtin.cpp`) - the only place that decides what a build contains -
 * so a module the list does not name is not linked in at all, and the failure
 * is a linker error naming the module rather than a runtime surprise. */
void register_builtin_modules();

}  // namespace brain

#endif  /* BRAINEMU_BUILTIN_BUILTIN_H */
