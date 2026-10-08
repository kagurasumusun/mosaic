/* The ELF32 loader: registration, and nothing else.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#ifndef BRAINEMU_LOADERS_ELF32_H
#define BRAINEMU_LOADERS_ELF32_H

namespace brain {

/* Adds the loader to the registry.  Called once, by the build's module list
 * (`builtin/builtin.cpp`), which is the only place that decides what this build
 * contains. */
void register_elf32_loader();

}  // namespace brain

#endif  /* BRAINEMU_LOADERS_ELF32_H */
