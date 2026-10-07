/* brain-emu -- emulator core with pluggable CPUs
 * Public C API: an embeddable, allocation-light interface to the core.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The emulator is a core plus modules, and this header is the core's whole
 * surface: an address space, one CPU chosen from the modules this build
 * contains, image loading, devices, and the run loop.  The primary module is
 * the ARM926EJ-S (ARMv5TEJ) core of an i.MX28, which is what a default
 * `brain_emu_create()` gives you; `brain_config_t::cpu_id` picks another one,
 * and the `brain_module_*` calls list what exists.
 *
 * The API is deliberately small and C-compatible: everything the core can do
 * that an embedder needs (load an image, poke registers, run for a while, read
 * counters, hook a device) is here, and nothing that would tie a caller to the
 * internal layout is.  Handles are opaque; the calls are not thread-safe per
 * handle (one CPU = one thread, as the core keeps no locks).
 *
 * Register numbering is the CPU module's: 0..reg_count-1 are its general
 * registers, and `brain_cpu_status_reg()` names the flags/mode word (16 for the
 * ARM module, which is also BRAIN_REG_CPSR).  A caller that wants to be
 * CPU-agnostic asks rather than assumes.
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
  const char* cpu_id;     /* CPU module id, e.g. "arm926"; NULL = the default */
  const char* cpu_options;/* "key=value;key=value" handed to the CPU module   */
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
  uint64_t flushes;       /* code cache flushes                               */
  uint64_t smc_flushes;   /* flushes caused by a store into a code page       */
  uint64_t tlb_hits, tlb_miss, walks, page_faults;
  uint64_t code_bytes;    /* bytes tracked as code by the self-modify guard   */
  uint64_t branch_taken, branch_not;
  uint64_t exc[BRAIN_EXC_COUNT];
} brain_stats_t;

/* ---- modules ------------------------------------------------------------ */

/* What this build can do, without creating a machine.  `brain_module_*` are
 * indexed 0..brain_module_count()-1, grouped by kind (every CPU, then the
 * loaders, then the devices, then the monitors); a CPU module is what a machine
 * can be built around, and the other kinds are selected by id too - a loader by
 * `brain_load_*`, a device by `brain_add_device_module`, a monitor by
 * `brain_add_monitor_module`. */
uint32_t brain_module_count(void);
const char* brain_module_id(uint32_t index);
const char* brain_module_name(uint32_t index);
const char* brain_module_kind(uint32_t index);        /* "cpu", "loader", ... */
const char* brain_module_description(uint32_t index);
/* The id a default machine uses: the build's primary CPU. */
const char* brain_default_cpu_id(void);

/* The CPU a machine was built around, and what the core can say about it
 * without knowing its instruction set. */
const char* brain_cpu_id(const brain_emu_t* e);
const char* brain_cpu_name(const brain_emu_t* e);
const char* brain_cpu_isa(const brain_emu_t* e);
uint32_t brain_cpu_reg_count(const brain_emu_t* e);
/* Register indices: -1 when the CPU has no such register. */
int brain_cpu_pc_reg(const brain_emu_t* e);
int brain_cpu_status_reg(const brain_emu_t* e);
int brain_cpu_saved_status_reg(const brain_emu_t* e);
int brain_cpu_sp_reg(const brain_emu_t* e);
/* Non-zero when the CPU has a second instruction set bit 0 of the PC selects. */
int brain_cpu_alternate_state(const brain_emu_t* e);

/* Hand a module an option the core's config does not carry ("key=value" is the
 * CLI spelling).  Returns 1 if the module accepted it. */
int brain_set_option(brain_emu_t* e, const char* key, const char* value);

/* ---- lifecycle ---------------------------------------------------------- */

/* Returns NULL on failure.  The reason is available afterwards from
 * brain_last_error(NULL) - a handle that was never created cannot carry it, and
 * "no CPU module named X" is worth reading. */
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

/* Build a *device module* into the machine, by id - the same thing the config's
 * `devices[]` does, for a caller that assembles a machine after creating it.
 * `base` is the device's window; `has_base == 0` means "the module's own
 * default address".  Returns 0, or -1 with the error naming the ids that exist.
 * Devices and monitors built this way live in the machine. */
int brain_add_device_module(brain_emu_t* e, const char* id, uint32_t base,
                            int has_base);
int brain_add_monitor_module(brain_emu_t* e, const char* id);

/* What the machine's devices and monitors are, and what they have to say.  The
 * core does not know any field: a device or monitor describes itself (see
 * Device::field_* in the C++ headers), and this is how a tool - a stats print, a
 * debugger, a GUI - collects the description without a core change. */
uint32_t brain_device_count(const brain_emu_t* e);
const char* brain_device_id(const brain_emu_t* e, uint32_t index);
uint32_t brain_device_base(const brain_emu_t* e, uint32_t index);
uint32_t brain_device_field_count(const brain_emu_t* e, uint32_t device);
const char* brain_device_field_name(const brain_emu_t* e, uint32_t device,
                                    uint32_t field);
const char* brain_device_field_value(const brain_emu_t* e, uint32_t device,
                                     uint32_t field);
uint32_t brain_monitor_count(const brain_emu_t* e);
const char* brain_monitor_id(const brain_emu_t* e, uint32_t index);
uint32_t brain_monitor_field_count(const brain_emu_t* e, uint32_t monitor);
const char* brain_monitor_field_name(const brain_emu_t* e, uint32_t monitor,
                                     uint32_t field);
const char* brain_monitor_field_value(const brain_emu_t* e, uint32_t monitor,
                                      uint32_t field);

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
