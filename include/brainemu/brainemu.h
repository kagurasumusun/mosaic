/* brain-emu -- ARM926EJ-S (ARMv5TEJ) core emulator
 * Public C API: an embeddable, allocation-light interface to the core.
 * SPDX-file-note: see LICENSE.
 *
 * The API is deliberately small and C-compatible: everything the core can do
 * that an embedder needs (load an image, poke registers, run for a while, read
 * counters, hook a device) is here, and nothing that would tie a caller to the
 * internal layout is.  Handles are opaque; the calls are not thread-safe per
 * handle (one CPU = one thread, as the core keeps no locks).
 */
#ifndef BRAINEMU_BRAINEMU_H
#define BRAINEMU_BRAINEMU_H

#include <stddef.h>
#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

typedef struct brain_emu brain_emu_t;

/* Register selectors for brain_get_reg / brain_set_reg.  0..15 are R0-R15,
 * 16 is CPSR and 17 the SPSR of the current mode. */
#define BRAIN_REG_CPSR 16
#define BRAIN_REG_SPSR 17

/* Exception numbers, matching the core's ordering. */
enum {
  BRAIN_EXC_NONE = 0,
  BRAIN_EXC_RESET,
  BRAIN_EXC_UNDEF,
  BRAIN_EXC_SWI,
  BRAIN_EXC_PABORT,
  BRAIN_EXC_DABORT,
  BRAIN_EXC_IRQ,
  BRAIN_EXC_FIQ,
  BRAIN_EXC_COUNT
};

/* Device callbacks, as seen by the bus.  `size` is 1, 2 or 4; a read returns
 * the value, a write ignores it.  Addresses are guest physical. */
typedef uint32_t (*brain_bus_read_t)(void* user, uint32_t addr, uint32_t size);
typedef void (*brain_bus_write_t)(void* user, uint32_t addr, uint32_t value,
                                  uint32_t size);

/* Everything that can be turned on without touching the core's internals.
 * Zero the struct and set the fields you care about; 0/NULL mean "default". */
typedef struct {
  uint32_t ram_base;      /* physical address of the RAM window (0 = default) */
  uint32_t ram_size;      /* bytes; 0 = 32 MiB                                */
  int use_jit;            /* ask the JIT for blocks (falls back if no backend)*/
  int big_endian_data;    /* BE8 data accesses                                */
  int high_vectors;       /* CP15 c1 V at reset: 0xFFFF0000 vectors           */
  int trace;              /* disassemble every instruction as it retires      */
  int cycle_model;        /* 0 = count instructions only (0 = off)            */
} brain_config_t;

/* Counters worth showing to a user; all are cumulative since the last
 * brain_emu_create() or brain_reset(). */
typedef struct {
  uint64_t insns;         /* instructions retired, both engines             */
  uint64_t insns_interp;
  uint64_t insns_jit;
  uint64_t cycles;
  uint64_t blocks;        /* JIT blocks built                                 */
  uint64_t links;         /* direct block-to-block links installed            */
  uint64_t flushes;       /* code cache flushes                               */
  uint64_t smc_flushes;   /* flushes caused by a store into a code page       */
  uint64_t tlb_hits, tlb_miss, walks, page_faults;
  uint64_t code_bytes;    /* bytes tracked as code by the self-modify guard   */
  uint64_t branch_taken, branch_not;
  uint64_t exc[BRAIN_EXC_COUNT];
} brain_stats_t;

/* ---- lifecycle ---------------------------------------------------------- */

/* Returns NULL on failure; call brain_last_error() for the reason. */
brain_emu_t* brain_emu_create(const brain_config_t* cfg);
void brain_emu_destroy(brain_emu_t* e);

/* Machine reset: registers, CPSR (SVC, IRQ/FIQ masked like the hardware),
 * CP15 to its reset values, TLB and code cache empty. */
void brain_reset(brain_emu_t* e);

const char* brain_last_error(const brain_emu_t* e);
const char* brain_version_string(void);

/* ---- loading ------------------------------------------------------------ */

/* Copy a raw image into guest memory at `addr`.  If `thumb` is non-zero the
 * CPU starts in Thumb state, which is how a boot ROM enters a payload. */
int brain_load_raw(brain_emu_t* e, const void* data, size_t size,
                   uint32_t addr, int thumb);

/* Load an ELF32 little-endian ARM executable (ET_EXEC only) from a buffer
 * already in the caller's memory.  Every PT_LOAD segment is copied to its
 * physical address; the bss part is zero-filled.  `*entry` receives e_entry,
 * with bit 0 preserved because that is how an ARM image marks a Thumb entry.
 * Returns 0 on success, -1 on any problem (brain_last_error() explains). */
int brain_load_elf32(brain_emu_t* e, const void* image, size_t size,
                     uint32_t* entry);

/* Map a device over `size` bytes at `base`.  Useful for a test harness that
 * wants an exit path: the callback can call brain_stop(). */
int brain_map_device(brain_emu_t* e, uint32_t base, uint32_t size,
                     brain_bus_read_t rd, brain_bus_write_t wr, void* user,
                     const char* name);

/* ---- running ------------------------------------------------------------ */

/* Run until `n` instructions have retired (0 = run forever, until
 * brain_stop()).  Returns the number of instructions actually retired. */
uint64_t brain_run_insns(brain_emu_t* e, uint64_t n);
/* Run until `max_cycles` have been charged (0 = unlimited). */
uint64_t brain_run_cycles(brain_emu_t* e, uint64_t max_cycles);
/* Retire exactly one instruction: the debugger's single step. */
int brain_step(brain_emu_t* e);
/* Ask the run loop to stop at the next safe point (from a device callback). */
void brain_stop(brain_emu_t* e);

/* ---- state -------------------------------------------------------------- */

uint32_t brain_get_reg(brain_emu_t* e, int which);
int brain_set_reg(brain_emu_t* e, int which, uint32_t value);

/* Accesses as the *guest* makes them: through the bus, so a device hook sees
 * them, and an unmapped address takes a data abort in the machine.  That is
 * what a debugger wants, and what a --dump of an MMIO register needs. */
uint32_t brain_read8(brain_emu_t*, uint32_t addr);
uint32_t brain_read16(brain_emu_t*, uint32_t addr);
uint32_t brain_read32(brain_emu_t*, uint32_t addr);
void brain_write8(brain_emu_t*, uint32_t addr, uint32_t value);
void brain_write16(brain_emu_t*, uint32_t addr, uint32_t value);
void brain_write32(brain_emu_t*, uint32_t addr, uint32_t value);
/* Same shape but physical and side-effect free: RAM only, no device hooks, no
 * exceptions.  Use these to inspect memory without perturbing the machine. */
uint32_t brain_peek8(brain_emu_t*, uint32_t addr);
uint32_t brain_peek16(brain_emu_t*, uint32_t addr);
uint32_t brain_peek32(brain_emu_t*, uint32_t addr);
void brain_poke8(brain_emu_t*, uint32_t addr, uint32_t value);
void brain_poke16(brain_emu_t*, uint32_t addr, uint32_t value);
void brain_poke32(brain_emu_t*, uint32_t addr, uint32_t value);
/* Bulk copy between guest and host memory; RAM/ROM only, all-or-nothing, and
 * 0 on success. */
int brain_read_mem(brain_emu_t*, uint32_t addr, void* dst, size_t n);
int brain_write_mem(brain_emu_t*, uint32_t addr, const void* src, size_t n);

/* Non-zero when the core is holding an exception it has not finished taking. */
/* Drive the level-sensitive nIRQ/nFIQ inputs.  Like the pins, a line stays in
 * effect until the caller drops it: after a handler returns with the line still
 * high, the next step boundary re-enters the handler.  */
void brain_set_irq(brain_emu_t*, int level);
void brain_set_fiq(brain_emu_t*, int level);

int brain_pending_exception(const brain_emu_t* e);
const char* brain_exception_name(int exc);

void brain_get_stats(const brain_emu_t* e, brain_stats_t* out);

#if defined(__cplusplus)
}  /* extern "C" */
#endif

#endif  /* BRAINEMU_BRAINEMU_H */
