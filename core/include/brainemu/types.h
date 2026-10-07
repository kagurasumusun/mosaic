/* brain-emu -- ARM926EJ-S (ARMv5TEJ) core emulator
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#ifndef BRAINEMU_TYPES_H
#define BRAINEMU_TYPES_H

#include <cstddef>
#include <cstdint>

#if defined(__GNUC__) || defined(__clang__)
#define ALWAYS_INLINE inline __attribute__((always_inline))
#define NEVER_INLINE __attribute__((noinline))
#define BRAIN_LIKELY(x) __builtin_expect(!!(x), 1)
#define BRAIN_UNLIKELY(x) __builtin_expect(!!(x), 0)
#else
#define ALWAYS_INLINE inline
#define NEVER_INLINE
#define BRAIN_LIKELY(x) (x)
#define BRAIN_UNLIKELY(x) (x)
#endif

namespace brain {

using u8  = std::uint8_t;
using u16 = std::uint16_t;
using u32 = std::uint32_t;
using u64 = std::uint64_t;
using s8  = std::int8_t;
using s16 = std::int16_t;
using s32 = std::int32_t;
using s64 = std::int64_t;

/* Guest physical / virtual addresses are 32-bit on ARM926EJ-S. */
using GAddr = u32;

/* Result of a memory access performed by the core. */
enum class AccessResult : u32 {
  Ok = 0,
  /* Fault was taken by the core (already queued as an exception). */
  Fault = 1,
};

}  // namespace brain

#endif  /* BRAINEMU_TYPES_H */
