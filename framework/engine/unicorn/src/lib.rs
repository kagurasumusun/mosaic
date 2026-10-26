#![deny(unsafe_code)]
//! IA-32 (16/32-bit) is a separate backend pair from x86_64: a PC boots in
//! 16-bit real mode and reaches protected mode through its own `CR0` write, so
//! the mode word is part of the backend, not a per-run knob (ADR 0018).
//!
//! Two PC-platform services live here because they are backend facts:
//!
//! * [`PortBus`] — `IN`/`OUT` port I/O. A PC reaches its 8259/8254/16550/RTC
//!   only through the port space, which is not memory-mapped anywhere.
//! * [`UnicornX86::queue_interrupt`] — external interrupt delivery. Unicorn
//!   2.1.x has no injection API for any ISA, so the adapter performs the
//!   architectural entry sequence itself (IDT/IVT decode, frame push, `IF`/`TF`
//!   masking) against guest memory.
//!
//! Unicorn adapters: one instance owns one core (design §12.3). ARM A32 is
//! always built; RV32I joins behind the `arch-riscv` feature (design §12.3
//! rule 6: `arch_*` features narrow the ISA set). No native handles,
//! callbacks or vendor types are part of the public API: register numbers are
//! the ISA crates' Mosaic RegId vocabulary, mapped per backend here.

mod ownership;
mod shared;

use core::num::NonZeroU64;
use std::sync::Arc;

use mosaic_contract_cpu_engine::{
    CpuEngine, EngineBus, EngineBusFault, EngineCaps, EngineError, IrqLine, RegId, RunOutcome,
    StopReason,
};
use mosaic_core_types::{AccessSize, Addr, BusFault, Permissions};
use ownership::{Counters, OwnedUnicorn};
use unicorn_engine::{
    unicorn_const::{Arch, HookType, MemType, Mode, Prot},
    RegisterARM, RegisterARM64, RegisterX86, Unicorn,
};

#[cfg(feature = "arch-riscv")]
use unicorn_engine::RegisterRISCV;

pub use shared::SharedRam;

/// Deliberate safety limit on an individual synchronous run, not a timing model.
pub const MAX_QUANTUM: u64 = 1_000_000;
pub const PAGE_SIZE: u64 = 4096;

/// ARM physical IRQ input, line 0.
pub const ARM_IRQ: IrqLine = IrqLine(0);
/// ARM physical FIQ input, line 1.
pub const ARM_FIQ: IrqLine = IrqLine(1);
/// RISC-V machine-mode external interrupt input, line 0. Unicorn 2.1.x has no
/// public injection API for it either, so levels are recorded like ARM's.
#[cfg(feature = "arch-riscv")]
pub const RISCV_MEXT: IrqLine = IrqLine(0);

/// Per-instance backend data. The bus port is installed once, before any MMIO
/// region is mapped, and hooks reach it only through this owned data.
#[derive(Default)]
pub struct EngineData {
    counters: Counters,
    bus: Option<Box<dyn EngineBus>>,
    ports: Option<Box<dyn PortBus>>,
    interrupts: Option<Box<dyn InterruptService>>,
    fault: Option<EngineBusFault>,
    /// x86 descriptor tables recorded when the guest executes `LGDT`/`LIDT`.
    tables: DescriptorTables,
    /// Bounded ring of recent instructions as `(address, size)`, recorded only
    /// when a bring-up harness asks for it. A guest that faults leaves no
    /// other record of *where* it went, and a debugger is not always
    /// available; the size lets the caller decode what actually executed
    /// instead of what the file on disk says should be there.
    trace: Option<std::collections::VecDeque<(u64, u32, u64)>>,
    /// Why the last quantum stopped, when the backend said no in its own
    /// words. "Fault" is a category; `UC_ERR_WRITE_UNMAPPED` next to the address
    /// the guest used is a diagnosis.
    last_error: Option<String>,
    /// The last vector the interrupt service refused *and* the guest had no
    /// gate for, if any. The adapter delivers refused vectors through the
    /// guest's own IDT/IVT; this records the ones it could not place, which
    /// stop the quantum (ADR 0020).
    unclaimed: Option<u8>,
    /// The last CPU exception the adapter delivered itself, as
    /// `(vector, eip, cr2)`. The return address and `CR2` are the only record
    /// of *where* a guest faulted, and a boot that stops inside its own fault
    /// handler needs both to be diagnosable.
    exception: Option<(u8, u64, u64)>,
    /// The first access since the last quantum that hit unmapped memory, as
    /// `(is_write, address, size)`. `UC_ERR_READ_UNMAPPED` alone names the
    /// failure mode but not the pointer that caused it, and on a boot path the
    /// difference between a stray `0xff7c` and a stray `0x1000ff7c` is the whole
    /// diagnosis.
    unmapped: Option<(bool, u64, usize)>,
    /// Instruction budget for the current quantum on an x86 backend, counted
    /// by the code hook so the quantum ends on an instruction boundary.
    budget: Option<u64>,
    /// Set when the budget — not the guest and not the backend — ended the
    /// quantum. A `rep` instruction makes the count overshoot, so the stop
    /// reason cannot be inferred from the count alone.
    budget_stop: bool,
    /// Address of the instruction the core is about to execute, recorded by the
    /// budget hook. Unicorn raises a software `INT n`'s `UC_HOOK_INTR` *after*
    /// folding the return address past the `INT`, and an exception's *before*
    /// touching `EIP` at all; comparing this with `EIP` in the hook is what
    /// tells the two apart without disassembling the instruction stream.
    executing: Option<u64>,
    /// Watched write regions and the writes seen in them, as
    /// `(guest address, writer pc, size)`. Off unless asked for: the hook runs
    /// on every store.
    watch: Vec<(u64, u64)>,
    writes: Vec<(u64, u64, u32)>,
}

/// Engine-facing port I/O port (IA-32 `IN`/`OUT`).
///
/// Sizes are the widths Unicorn reports: 1, 2 or 4 bytes. A fault stops the
/// quantum and is recorded exactly like an MMIO fault, so a platform bug can
/// never be mistaken for a guest hang.
pub trait PortBus: Send {
    fn read(&mut self, port: u16, size: usize) -> Result<u32, BusFault>;
    fn write(&mut self, port: u16, size: usize, value: u32) -> Result<(), BusFault>;
}

/// CPU view handed to a software-interrupt service.
///
/// Register numbers are Mosaic [`RegId`] values, so a platform never sees a
/// vendor register enum. The adapter re-uses the same register map as the
/// backend, which keeps `INT`-level firmware emulation and normal execution in
/// agreement.
pub trait SoftwareInterruptCpu {
    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError>;
    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError>;
    fn mem_read(&self, address: u64, data: &mut [u8]) -> Result<(), EngineError>;
    fn mem_write(&mut self, address: u64, data: &[u8]) -> Result<(), EngineError>;
}

/// Services software interrupts (`INT n`) for the platform.
///
/// A PC's firmware interface *is* the software-interrupt table, so a machine
/// that boots real guest code needs somewhere to answer `INT 13h`/`INT 10h`
/// without a full BIOS ROM. Unicorn reports the `INT` through its interrupt
/// hook before any gate is entered; the service answers it and the guest
/// resumes at the instruction after the `INT` (ADR 0019).
pub trait InterruptService: Send {
    /// Answer `vector`. Returning `false` leaves the vector to the caller's
    /// own gate delivery, which is how a guest-installed handler keeps working.
    fn service(&mut self, vector: u8, cpu: &mut dyn SoftwareInterruptCpu) -> bool;
}

/// Last observed `GDTR`/`IDTR` values: `(base, limit)`.
///
/// The binding's `reg_read_long` uses a 10-byte buffer for these registers
/// while the C API writes a 24-byte `uc_x86_mmr`, so the adapter cannot read
/// them back; it records the loads instead (ADR 0018).
#[derive(Clone, Copy, Debug, Default)]
pub struct DescriptorTables {
    pub gdt: Option<(u64, u64)>,
    pub idt: Option<(u64, u64)>,
}

/// Which ISA/mode the owned Unicorn instance executes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Backend {
    Arm,
    /// ARMv7-M class core (Cortex-M3): T32 execution from reset.
    ArmMClass,
    Arm64,
    X86_64,
    /// IA-32 in 16-bit real mode, the state a PC boots in.
    X86Real,
    /// IA-32 with a 32-bit default operand size; `CR0.PE` stays guest-owned.
    X86Protected,
    /// IA-32 that *resets* into real mode but runs on the 32-bit mode word.
    /// `UC_MODE_16`'s `uc_emu_start` converts its `begin` argument into a
    /// 16-bit IP (`ip = begin - cs * 16`), so a resumed context can never
    /// address above 64 KiB — which a protected-mode decompressor does on its
    /// first instruction (ADR 0020). This backend picks the mode word only for
    /// that reason; the reset state is a real PC's.
    X86Boot,
    #[cfg(feature = "arch-riscv")]
    RiscV32,
}

/// uc_riscv_reg lists X0..X31 contiguously; the table makes the mapping
/// explicit and testable instead of relying on arithmetic over enum values.
#[cfg(feature = "arch-riscv")]
const RISCV_GENERAL: [RegisterRISCV; 32] = [
    RegisterRISCV::X0,
    RegisterRISCV::X1,
    RegisterRISCV::X2,
    RegisterRISCV::X3,
    RegisterRISCV::X4,
    RegisterRISCV::X5,
    RegisterRISCV::X6,
    RegisterRISCV::X7,
    RegisterRISCV::X8,
    RegisterRISCV::X9,
    RegisterRISCV::X10,
    RegisterRISCV::X11,
    RegisterRISCV::X12,
    RegisterRISCV::X13,
    RegisterRISCV::X14,
    RegisterRISCV::X15,
    RegisterRISCV::X16,
    RegisterRISCV::X17,
    RegisterRISCV::X18,
    RegisterRISCV::X19,
    RegisterRISCV::X20,
    RegisterRISCV::X21,
    RegisterRISCV::X22,
    RegisterRISCV::X23,
    RegisterRISCV::X24,
    RegisterRISCV::X25,
    RegisterRISCV::X26,
    RegisterRISCV::X27,
    RegisterRISCV::X28,
    RegisterRISCV::X29,
    RegisterRISCV::X30,
    RegisterRISCV::X31,
];

/// Opaque CPU register and backend execution context captured by Unicorn.
pub struct UnicornContext(pub(crate) unicorn_engine::Context);

/// All backend logic lives here; the public structs are thin per-ISA wrappers
/// created by [`unicorn_wrapper`].
struct Common {
    backend: Backend,
    inner: OwnedUnicorn,
    /// Native RAM/ROM pages eligible for loader access.
    mappings: Vec<(u64, u64)>,
    /// Callback-backed MMIO pages; never reachable through loader access.
    mmio: Vec<(u64, u64)>,
    /// Host-backed shared pages (DMI). Each entry is the guest address, the
    /// size and the authoritative buffer. The engine keeps a writable mirror
    /// of the same bytes; `uc_mem_map_ptr` maps read-only towards the guest
    /// on Unicorn 2.1.x, so the composition root synchronises the mirrors at
    /// deterministic slice boundaries (ADR 0010).
    shared: Vec<(u64, u64, Arc<SharedRam>)>,
    /// External interrupt edges in delivery order. Unicorn 2.1.x exposes no
    /// public IRQ injection API, so levels are recorded, not injected
    /// (ADR 0009); the edge log stays observable for wiring tests.
    irq_edges: Vec<(IrqLine, bool)>,
    /// IA-32 vectors awaiting delivery, in request order (ADR 0018). A vector
    /// stays queued while `EFLAGS.IF` is clear, exactly as a real INTR does.
    pending_interrupts: std::collections::VecDeque<u8>,
}

impl Common {
    fn new(backend: Backend) -> Result<Self, EngineError> {
        let (arch, mode) = match backend {
            Backend::Arm => (Arch::ARM, Mode::ARM | Mode::LITTLE_ENDIAN),
            // MCLASS selects the Cortex-M3 CPU model, which is Thumb-only and
            // enters T32 from reset (ADR 0012).
            Backend::ArmMClass => (Arch::ARM, Mode::MCLASS | Mode::LITTLE_ENDIAN),
            Backend::Arm64 => (Arch::ARM64, Mode::LITTLE_ENDIAN),
            Backend::X86_64 => (Arch::X86, Mode::MODE_64),
            Backend::X86Real => (Arch::X86, Mode::MODE_16 | Mode::LITTLE_ENDIAN),
            Backend::X86Protected | Backend::X86Boot => {
                (Arch::X86, Mode::MODE_32 | Mode::LITTLE_ENDIAN)
            }
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => (Arch::RISCV, Mode::MODE_32 | Mode::LITTLE_ENDIAN),
        };
        let mut uc = Unicorn::new_with_data(arch, mode, EngineData::default())
            .map_err(|_| EngineError::Backend)?;
        // Unicorn registers `emu_start`'s `until` address as an exit and, at the
        // end of every quantum, invalidates the translation block covering
        // `until - 1` (`tcg_cpu_exec` -> `uc_invalidate_tb`). This adapter never
        // stops on an address, so it passes `u64::MAX`, and the invalidation then
        // asks `get_page_addr_code` to translate `0xfffffffffffffffe`: with guest
        // paging on, that address-space walk faults, `raise_interrupt2` runs
        // `check_exception` — latching `old_exception = #PF` — and the exception
        // is swallowed by the invalidation's own `setjmp`, so nothing ever clears
        // it. The second quantum latches `#DF`, and from then on
        // `check_exception` turns every guest exception into a triple fault
        // (`EXCP_HLT`) before the adapter's `UC_HOOK_INTR` callback is reached:
        // the guest freezes mid-instruction and each quantum retires one
        // instruction. `UC_CTL_UC_USE_EXITS` removes the mechanism: `until` is
        // ignored and only addresses registered through `ctl_set_exits` are
        // invalidated, of which this adapter registers none.
        uc.ctl_exits_enable().map_err(|_| EngineError::Backend)?;
        // Capture-free: no Rc/host reference can escape the engine through a hook.
        // Native count and MAX_QUANTUM bound the addition below to <= 1_000_000.
        uc.add_code_hook(1, 0, |uc, address, _| {
            // `emu_start`'s own count can stop inside a repeat-prefixed string
            // instruction, and a context stopped there cannot be resumed
            // (ADR 0020). The budget is therefore counted here and the stop is
            // deferred past any `rep movs`: the string instruction retires, and
            // the quantum ends on the next instruction boundary.
            // The hook runs *before* the instruction retires, so the budget is
            // spent by refusing to enter the (limit + 1)-th instruction: the
            // count then stays exactly `limit` retired instructions. A `rep`
            // string instruction at the boundary is let in whole and overshoots
            // by one, which keeps the trap flag free of half-done copies.
            let stop = {
                let rep = x86_is_rep_string(uc, address);
                let data = uc.get_data_mut();
                data.executing = Some(address);
                data.counters.instructions += 1;
                match data.budget {
                    Some(limit) if data.counters.instructions > limit && !rep => {
                        data.counters.instructions -= 1;
                        data.budget = None;
                        data.budget_stop = true;
                        true
                    }
                    _ => false,
                }
            };
            if stop {
                let _ = uc.emu_stop();
            }
        })
        .map_err(|_| EngineError::Backend)?;
        // One hook for every backend: the address a faulting access used is
        // the single most useful number after a stop, and Unicorn's error code
        // does not carry it.
        uc.add_mem_hook(
            HookType::MEM_UNMAPPED,
            1,
            0,
            |uc, kind, address, size, _| {
                let write = matches!(
                    kind,
                    MemType::WRITE | MemType::WRITE_UNMAPPED | MemType::WRITE_PROT
                );
                let data = uc.get_data_mut();
                if data.unmapped.is_none() {
                    data.unmapped = Some((write, address, size));
                }
                // Never handled: let the backend raise its own fault.
                false
            },
        )
        .map_err(|_| EngineError::Backend)?;
        if matches!(backend, Backend::X86Boot) {
            // `MODE_32` exists for guests that are already protected, so it
            // arms `CR0.PE` and leaves every segment cache zeroed by the
            // memset. Undo both: `cpu_x86_load_seg` with `PE = 0` recomputes
            // base = selector * 16, limit 0xFFFF and a 16-bit code segment,
            // which is exactly what a PC resets into.
            uc.reg_write(RegisterX86::CR0 as i32, 0)
                .map_err(|_| EngineError::Backend)?;
            for segment in [
                RegisterX86::CS,
                RegisterX86::DS,
                RegisterX86::ES,
                RegisterX86::SS,
                RegisterX86::FS,
                RegisterX86::GS,
            ] {
                uc.reg_write(segment as i32, 0)
                    .map_err(|_| EngineError::Backend)?;
            }
        }
        if matches!(backend, Backend::ArmMClass) {
            // M-profile reset loads the T bit from the vector table at 0x0
            // (unmapped at construction time), so the core wakes executing
            // A32 and faults on every T32 opcode. UC_MODE_THUMB cannot help:
            // unicorn 2.1.x sets uc->thumb there but never reads it back.
            // A CPSR write is unusable here: reset leaves uncached_cpsr in
            // SVC mode even on M-profile, and cpsr_write with the full mask
            // then performs an invalid bank switch. The R15 write path is
            // the one that only touches pc/regs[15]/thumb, so an odd dummy
            // value asserts T32 without disturbing anything else; run()
            // keeps it sticky through begin = pc | 1.
            uc.reg_write(RegisterARM::PC as i32, 1)
                .map_err(|_| EngineError::Backend)?;
        }
        if matches!(
            backend,
            Backend::X86Real | Backend::X86Protected | Backend::X86Boot
        ) {
            // LGDT/LIDT are at least five bytes, so the 16-bit boot stub's
            // short instructions never reach the decoder, and the whole hook
            // switches off once both tables are known.
            uc.add_code_hook(1, 0, |uc, address, size| {
                // `LGDT`/`LIDT` are `0F 01` plus a ModRM byte at minimum, so a
                // shorter instruction cannot be one. The tables are observed for
                // the whole run rather than until they are first seen: a kernel
                // loads `GDTR` several times and `IDTR` again after the early
                // stubs, and a stale base delivers faults into the wrong table.
                if size >= 3 {
                    x86_observe_descriptor_load(uc, address);
                }
            })
            .map_err(|_| EngineError::Backend)?;
            // Port I/O is the only way a PC reaches its PIC/PIT/RTC/UART. Both
            // hooks share one port so IN and OUT cannot disagree.
            uc.add_insn_in_hook(|uc, port, size| {
                let Ok(port) = u16::try_from(port) else {
                    return stop_with_bus_fault(uc, u64::from(port), size, BusFault::Unmapped)
                        as u32;
                };
                let outcome = uc
                    .get_data_mut()
                    .ports
                    .as_mut()
                    .ok_or(BusFault::Unmapped)
                    .and_then(|bus| bus.read(port, size));
                match outcome {
                    Ok(value) => value,
                    Err(fault) => stop_with_bus_fault(uc, u64::from(port), size, fault) as u32,
                }
            })
            .map_err(|_| EngineError::Backend)?;
            uc.add_intr_hook(|uc, vector| {
                // Take the service out of the engine data for the duration of
                // the callback: the view handed to it borrows the same Unicorn
                // handle, so holding both would alias. Same pattern as the
                // runtime device arena.
                let mut service = uc.get_data_mut().interrupts.take();
                let mut handled = false;
                if let (Some(service), Ok(vector)) = (service.as_mut(), u8::try_from(vector)) {
                    let mut view = InterruptCpuView { uc };
                    handled = service.service(vector, &mut view);
                }
                uc.get_data_mut().interrupts = service;
                if !handled {
                    // Nothing claimed the vector. A software interrupt belongs
                    // to the firmware service; anything it declines is a CPU
                    // exception or a guest-installed handler's vector, and
                    // Unicorn never enters either gate on its own.
                    if let Ok(vector) = u8::try_from(vector) {
                        x86_take_exception(uc, vector);
                    } else {
                        let _ = uc.emu_stop();
                    }
                }
            })
            .map_err(|_| EngineError::Backend)?;
            uc.add_insn_out_hook(|uc, port, size, value| {
                let Ok(port) = u16::try_from(port) else {
                    stop_with_bus_fault(uc, u64::from(port), size, BusFault::Unmapped);
                    return;
                };
                let outcome = uc
                    .get_data_mut()
                    .ports
                    .as_mut()
                    .ok_or(BusFault::Unmapped)
                    .and_then(|bus| bus.write(port, size, value));
                if let Err(fault) = outcome {
                    stop_with_bus_fault(uc, u64::from(port), size, fault);
                }
            })
            .map_err(|_| EngineError::Backend)?;
        }
        Ok(Self {
            backend,
            inner: OwnedUnicorn(uc),
            mappings: Vec::new(),
            mmio: Vec::new(),
            shared: Vec::new(),
            irq_edges: Vec::new(),
            pending_interrupts: std::collections::VecDeque::new(),
        })
    }

    /// Install the port I/O port (`IN`/`OUT`). Re-attaching fails for the same
    /// reason as [`Common::attach_bus`]: stale hooks must not alias a new bus.
    fn attach_ports(&mut self, ports: Box<dyn PortBus>) -> Result<(), EngineError> {
        let data = self.inner.0.get_data_mut();
        if data.ports.is_some() {
            return Err(EngineError::BusAlreadyAttached);
        }
        data.ports = Some(ports);
        Ok(())
    }

    /// Install the callback bus used by every MMIO region. One port serves all
    /// regions; the port resolves guest addresses itself. Re-attaching fails so
    /// stale callbacks can never alias a new bus.
    fn attach_bus(&mut self, bus: Box<dyn EngineBus>) -> Result<(), EngineError> {
        if self.inner.0.get_data().bus.is_some() {
            return Err(EngineError::BusAlreadyAttached);
        }
        self.inner.0.get_data_mut().bus = Some(bus);
        Ok(())
    }

    /// Map a page-aligned callback region. Guest accesses inside it are routed
    /// to the attached bus; a bus fault stops emulation and is retained for
    /// [`Common::take_bus_fault`].
    ///
    /// Unicorn hands its MMIO callbacks region-relative offsets, so each pair
    /// of closures captures only the region's Copy base address and re-adds it
    /// before reaching the bus, which speaks absolute guest addresses.
    fn map_mmio(&mut self, base: Addr, size: u64) -> Result<(), EngineError> {
        if self.inner.0.get_data().bus.is_none() {
            return Err(EngineError::BusNotAttached);
        }
        self.validate_region(base, size)?;
        let region_base = base.0;
        self.inner
            .0
            .mmio_map(
                base.0,
                size,
                Some(
                    move |uc: &mut Unicorn<'_, EngineData>, offset: u64, width: usize| {
                        let Some(address) = region_base.checked_add(offset) else {
                            return stop_with_bus_fault(uc, u64::MAX, width, BusFault::Unmapped);
                        };
                        mmio_read_at(uc, address, width)
                    },
                ),
                Some(
                    move |uc: &mut Unicorn<'_, EngineData>,
                          offset: u64,
                          width: usize,
                          value: u64| {
                        let Some(address) = region_base.checked_add(offset) else {
                            stop_with_bus_fault(uc, u64::MAX, width, BusFault::Unmapped);
                            return;
                        };
                        mmio_write_at(uc, address, width, value);
                    },
                ),
            )
            .map_err(|_| EngineError::Backend)?;
        self.mmio.push((base.0, size));
        Ok(())
    }

    /// Map a host-backed shared region (design §12.3 rule 1).
    ///
    /// The engine receives a writable private mirror of the page; the
    /// authoritative buffer stays with the composition root, which copies the
    /// bytes in and out at deterministic slice boundaries. See [`SharedRam`]
    /// for why a direct pointer map cannot be used on Unicorn 2.1.x.
    fn map_shared_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
        ram: Arc<SharedRam>,
    ) -> Result<(), EngineError> {
        self.validate_region(base, size)?;
        if size > ram.len() as u64 {
            return Err(EngineError::InvalidMapping);
        }
        // The mirror follows the guest permissions. The synchronisation path
        // writes it through the loader API, which ignores guest permissions,
        // so a read-only shared page still synchronises correctly.
        self.inner
            .0
            .mem_map(base.0, size, prot_of(permissions))
            .map_err(|_| EngineError::Backend)?;
        self.mappings.push((base.0, size));
        self.shared.push((base.0, size, ram));
        Ok(())
    }

    /// Copy the authoritative shared buffers into this engine's mirrors.
    /// Called before a core slice so the core observes the latest values.
    fn sync_shared_in(&mut self) -> Result<(), EngineError> {
        for (base, size, ram) in &self.shared {
            let length = usize::try_from(*size).map_err(|_| EngineError::InvalidMapping)?;
            let mut bytes = vec![0u8; length];
            ram.read(0, &mut bytes)?;
            self.inner
                .0
                .mem_write(*base, &bytes)
                .map_err(|_| EngineError::Backend)?;
        }
        Ok(())
    }

    /// Copy this engine's shared mirrors back into the authoritative
    /// buffers. Called after a core slice so the next core observes them.
    fn sync_shared_out(&mut self) -> Result<(), EngineError> {
        for (base, size, ram) in &self.shared {
            let length = usize::try_from(*size).map_err(|_| EngineError::InvalidMapping)?;
            let mut bytes = vec![0u8; length];
            self.inner
                .0
                .mem_read(*base, &mut bytes)
                .map_err(|_| EngineError::Backend)?;
            ram.write(0, &bytes)?;
        }
        Ok(())
    }

    /// Take and clear the first bus fault recorded since the previous call.
    fn take_bus_fault(&mut self) -> Option<EngineBusFault> {
        self.inner.0.get_data_mut().fault.take()
    }

    fn irq_edges(&self) -> &[(IrqLine, bool)] {
        &self.irq_edges
    }

    /// Most recently recorded level of one line, if it has ever been driven.
    fn irq_level(&self, line: IrqLine) -> Option<bool> {
        self.irq_edges
            .iter()
            .rev()
            .find(|(candidate, _)| *candidate == line)
            .map(|(_, high)| *high)
    }

    fn validate_region(&self, base: Addr, size: u64) -> Result<(), EngineError> {
        if size == 0
            || base.0 % PAGE_SIZE != 0
            || size % PAGE_SIZE != 0
            || u128::from(base.0) + u128::from(size) > (1u128 << 32)
        {
            return Err(EngineError::InvalidMapping);
        }
        let end = base.0 + size;
        let overlaps = self
            .mappings
            .iter()
            .chain(self.mmio.iter())
            .any(|&(start, len)| base.0 < start + len && start < end)
            || self
                .shared
                .iter()
                .any(|&(start, len, _)| base.0 < start + len && start < end);
        if overlaps {
            return Err(EngineError::Overlap);
        }
        Ok(())
    }

    fn register_id(&self, reg: RegId) -> Result<i32, EngineError> {
        match self.backend {
            Backend::Arm | Backend::ArmMClass => {
                let mapped = match reg.0 {
                    0 => RegisterARM::R0,
                    1 => RegisterARM::R1,
                    2 => RegisterARM::R2,
                    3 => RegisterARM::R3,
                    4 => RegisterARM::R4,
                    5 => RegisterARM::R5,
                    6 => RegisterARM::R6,
                    7 => RegisterARM::R7,
                    8 => RegisterARM::R8,
                    9 => RegisterARM::R9,
                    10 => RegisterARM::R10,
                    11 => RegisterARM::R11,
                    12 => RegisterARM::R12,
                    13 => RegisterARM::SP,
                    14 => RegisterARM::LR,
                    15 => RegisterARM::PC,
                    _ => return Err(EngineError::InvalidRegister),
                };
                Ok(mapped as i32)
            }
            Backend::Arm64 => {
                let mapped = match reg.0 {
                    0 => RegisterARM64::X0,
                    1 => RegisterARM64::X1,
                    2 => RegisterARM64::X2,
                    3 => RegisterARM64::X3,
                    4 => RegisterARM64::X4,
                    5 => RegisterARM64::X5,
                    6 => RegisterARM64::X6,
                    7 => RegisterARM64::X7,
                    8 => RegisterARM64::X8,
                    9 => RegisterARM64::X9,
                    10 => RegisterARM64::X10,
                    11 => RegisterARM64::X11,
                    12 => RegisterARM64::X12,
                    13 => RegisterARM64::X13,
                    14 => RegisterARM64::X14,
                    15 => RegisterARM64::X15,
                    16 => RegisterARM64::X16,
                    17 => RegisterARM64::X17,
                    18 => RegisterARM64::X18,
                    19 => RegisterARM64::X19,
                    20 => RegisterARM64::X20,
                    21 => RegisterARM64::X21,
                    22 => RegisterARM64::X22,
                    23 => RegisterARM64::X23,
                    24 => RegisterARM64::X24,
                    25 => RegisterARM64::X25,
                    26 => RegisterARM64::X26,
                    27 => RegisterARM64::X27,
                    28 => RegisterARM64::X28,
                    29 => RegisterARM64::FP,
                    30 => RegisterARM64::LR,
                    31 => RegisterARM64::SP,
                    32 => RegisterARM64::PC,
                    _ => return Err(EngineError::InvalidRegister),
                };
                Ok(mapped as i32)
            }
            Backend::X86_64 => {
                let mapped = match reg.0 {
                    0 => RegisterX86::RAX,
                    1 => RegisterX86::RBX,
                    2 => RegisterX86::RCX,
                    3 => RegisterX86::RDX,
                    4 => RegisterX86::RSI,
                    5 => RegisterX86::RDI,
                    6 => RegisterX86::RSP,
                    7 => RegisterX86::RBP,
                    8 => RegisterX86::R8,
                    9 => RegisterX86::R9,
                    10 => RegisterX86::R10,
                    11 => RegisterX86::R11,
                    12 => RegisterX86::R12,
                    13 => RegisterX86::R13,
                    14 => RegisterX86::R14,
                    15 => RegisterX86::R15,
                    16 => RegisterX86::RIP,
                    _ => return Err(EngineError::InvalidRegister),
                };
                Ok(mapped as i32)
            }
            Backend::X86Real | Backend::X86Protected | Backend::X86Boot => match reg.0 {
                // Mosaic ISA order EAX..EDI is the hardware encoding order.
                0 => Ok(RegisterX86::EAX as i32),
                1 => Ok(RegisterX86::ECX as i32),
                2 => Ok(RegisterX86::EDX as i32),
                3 => Ok(RegisterX86::EBX as i32),
                4 => Ok(RegisterX86::ESP as i32),
                5 => Ok(RegisterX86::EBP as i32),
                6 => Ok(RegisterX86::ESI as i32),
                7 => Ok(RegisterX86::EDI as i32),
                8 => Ok(RegisterX86::EIP as i32),
                9 => Ok(RegisterX86::EFLAGS as i32),
                10 => Ok(RegisterX86::CS as i32),
                11 => Ok(RegisterX86::DS as i32),
                12 => Ok(RegisterX86::ES as i32),
                13 => Ok(RegisterX86::FS as i32),
                14 => Ok(RegisterX86::GS as i32),
                15 => Ok(RegisterX86::SS as i32),
                16 => Ok(RegisterX86::CR0 as i32),
                17 => Ok(RegisterX86::CR2 as i32),
                18 => Ok(RegisterX86::CR3 as i32),
                19 => Ok(RegisterX86::CR4 as i32),
                _ => Err(EngineError::InvalidRegister),
            },
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => match reg.0 {
                // The arm covers X0..X31; the index is in bounds by the arm.
                0..=31 => Ok(RISCV_GENERAL[reg.0 as usize] as i32),
                32 => Ok(RegisterRISCV::PC as i32),
                _ => Err(EngineError::InvalidRegister),
            },
        }
    }

    fn check_span(&self, base: Addr, size: usize) -> Result<(), EngineError> {
        let end = u128::from(base.0) + size as u128;
        let inside =
            |start: u64, len: u64| base.0 >= start && end <= u128::from(start) + u128::from(len);
        if self.mappings.iter().any(|&(start, len)| inside(start, len))
            || self
                .shared
                .iter()
                .any(|&(start, len, _)| inside(start, len))
        {
            Ok(())
        } else {
            Err(EngineError::Unmapped)
        }
    }

    fn run(&mut self, instructions: NonZeroU64) -> RunOutcome {
        let budget = instructions.get();
        let outcome = |reason, instruction_hooks| RunOutcome {
            reason,
            instruction_hooks,
        };
        if budget > MAX_QUANTUM {
            return outcome(StopReason::CapabilityMissing, 0);
        }
        let begin = match self.backend {
            Backend::Arm => {
                let Ok(pc) = self.inner.0.reg_read(RegisterARM::PC) else {
                    return outcome(StopReason::Fault, 0);
                };
                // Unicorn takes Thumb state from bit 0 of begin. Preserve it on
                // resume even though the conformance suite only covers A32.
                let Ok(cpsr) = self.inner.0.reg_read(RegisterARM::CPSR) else {
                    return outcome(StopReason::Fault, 0);
                };
                pc | ((cpsr >> 5) & 1)
            }
            // M-profile cores are always in T32. emu_start writes begin
            // back through the PC register path, whose bit 0 re-asserts
            // env->thumb on every resume (unicorn_arm.c reg_write case R15);
            // this keeps the construction-time CPSR.T sticky even if an
            // aligned PC write momentarily cleared it.
            Backend::ArmMClass => {
                let Ok(pc) = self.inner.0.reg_read(RegisterARM::PC) else {
                    return outcome(StopReason::Fault, 0);
                };
                pc | 1
            }
            Backend::Arm64 => {
                let Ok(pc) = self.inner.0.reg_read(RegisterARM64::PC) else {
                    return outcome(StopReason::Fault, 0);
                };
                pc
            }
            Backend::X86_64 => {
                let Ok(pc) = self.inner.0.reg_read(RegisterX86::RIP) else {
                    return outcome(StopReason::Fault, 0);
                };
                pc
            }
            Backend::X86Real | Backend::X86Protected | Backend::X86Boot => {
                // External interrupts are taken at the quantum boundary: the
                // only point where a frame can be pushed without fighting an
                // in-flight instruction (ADR 0018).
                if self.x86_deliver_pending().is_err() {
                    return outcome(StopReason::Fault, 0);
                }
                let Ok(pc) = self.inner.0.reg_read(RegisterX86::EIP) else {
                    return outcome(StopReason::Fault, 0);
                };
                // In *real* mode `EIP` is a 16-bit offset that wraps inside the
                // segment; a guest that has set `CR0.PE` executes 32-bit
                // offsets and must not be truncated. The kernel's decompressor
                // running at `10007Ch` is exactly the case that catches this.
                let protected = self
                    .inner
                    .0
                    .reg_read(RegisterX86::CR0 as i32)
                    .map(|cr0| cr0 & 1 != 0)
                    .unwrap_or(true);
                if matches!(self.backend, Backend::X86Real | Backend::X86Boot) && !protected {
                    // Real mode: EIP is a 16-bit offset inside the segment, and
                    // both mode words here take `begin` as an offset.
                    pc & 0xFFFF
                } else {
                    pc
                }
            }
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => {
                let Ok(pc) = self.inner.0.reg_read(RegisterRISCV::PC) else {
                    return outcome(StopReason::Fault, 0);
                };
                pc
            }
        };
        self.inner.0.get_data_mut().counters.instructions = 0;
        let x86 = matches!(
            self.backend,
            Backend::X86Real | Backend::X86Protected | Backend::X86Boot
        );
        if x86 {
            let data = self.inner.0.get_data_mut();
            data.budget = Some(budget);
            data.budget_stop = false;
        }
        // x86 passes no count: the code hook stops the quantum, which keeps the
        // guest on an instruction boundary even when the instruction is a
        // multi-megabyte `rep movs`.
        let count = if x86 { 0 } else { budget as usize };
        let result = self.inner.0.emu_start(begin, u64::MAX, 0, count);
        self.inner.0.get_data_mut().budget = None;
        let observed = self.inner.0.get_data().counters.instructions;
        if let Err(error) = &result {
            self.inner.0.get_data_mut().last_error = Some(format!("{error:?}"));
        }
        let budget_stop = self.inner.0.get_data().budget_stop;
        let reason = if result.is_err() || self.inner.0.get_data().fault.is_some() {
            StopReason::Fault
        } else if budget_stop || observed == budget {
            StopReason::BudgetExhausted
        } else {
            // A backend early stop is not proof of a precise WFI/halt reason.
            StopReason::CapabilityMissing
        };
        outcome(reason, observed)
    }

    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError> {
        self.inner
            .0
            .reg_read(self.register_id(reg)?)
            .map_err(|_| EngineError::Backend)
    }

    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError> {
        let id = self.register_id(reg)?;
        if value > u64::from(u32::MAX) {
            return Err(EngineError::InvalidValue);
        }
        // 32-bit backends only; PC writes must stay instruction-aligned.
        // A32 keeps the 4-byte rule. T32 is reachable only through the
        // dedicated M-class construction (ADR 0012): on the A32 backend
        // neither a CPSR.T write nor an odd emu_start begin address switches
        // Unicorn 2.1.x into Thumb (verified empirically on 2.1.5), so an
        // odd PC is rejected instead of silently executing as A32. M-profile
        // cores are always in T32 and use halfword alignment.
        let pc_id = match self.backend {
            Backend::Arm | Backend::ArmMClass => 15,
            Backend::Arm64 => 32,
            Backend::X86_64 => 16,
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => 32,
            Backend::X86Real | Backend::X86Protected | Backend::X86Boot => 8,
        };
        if reg.0 == pc_id {
            let aligned = match self.backend {
                Backend::Arm | Backend::Arm64 => value % 4 == 0,
                Backend::ArmMClass => value % 2 == 0,
                Backend::X86_64 => true, // x86 instructions are byte aligned
                #[cfg(feature = "arch-riscv")]
                Backend::RiscV32 => value % 4 == 0,
                // x86 instructions are variable length; any EIP is legal.
                Backend::X86Real | Backend::X86Protected | Backend::X86Boot => true,
            };
            if !aligned {
                return Err(EngineError::InvalidValue);
            }
        }
        self.inner
            .0
            .reg_write(id, value)
            .map_err(|_| EngineError::Backend)
    }

    fn map_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
    ) -> Result<(), EngineError> {
        self.validate_region(base, size)?;
        self.inner
            .0
            .mem_map(base.0, size, prot_of(permissions))
            .map_err(|_| EngineError::Backend)?;
        self.mappings.push((base.0, size));
        Ok(())
    }

    fn write_memory(&mut self, base: Addr, data: &[u8]) -> Result<(), EngineError> {
        if data.is_empty() {
            return Ok(());
        }
        self.check_span(base, data.len())?;
        self.inner
            .0
            .mem_write(base.0, data)
            .map_err(|_| EngineError::Backend)?;
        // Loader writes bypass guest protection, but must not leave stale TBs.
        self.inner
            .0
            .ctl_remove_cache(base.0, base.0 + data.len() as u64)
            .map_err(|_| EngineError::Backend)
    }

    fn read_memory(&self, base: Addr, data: &mut [u8]) -> Result<(), EngineError> {
        if data.is_empty() {
            return Ok(());
        }
        self.check_span(base, data.len())?;
        self.inner
            .0
            .mem_read(base.0, data)
            .map_err(|_| EngineError::Backend)
    }

    fn save_context(&self) -> Result<UnicornContext, EngineError> {
        let mut ctx = self
            .inner
            .0
            .context_init()
            .map_err(|_| EngineError::Backend)?;
        self.inner
            .0
            .context_save(&mut ctx)
            .map_err(|_| EngineError::Backend)?;
        Ok(UnicornContext(ctx))
    }

    fn restore_context(&mut self, context: &UnicornContext) -> Result<(), EngineError> {
        self.inner
            .0
            .context_restore(&context.0)
            .map_err(|_| EngineError::Backend)
    }

    fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError> {
        let supported = match self.backend {
            // ARMv7-A / ARM64 exposes the physical IRQ and FIQ inputs; any other
            // number is a composition error, not a guest fault.
            Backend::Arm | Backend::Arm64 => line.0 <= 1,
            // ARMv7-M routes external interrupts through the NVIC; the core
            // itself exposes only the (maskable) exception input.
            Backend::ArmMClass => line.0 == 0,
            // x86_64 takes INTR on line 0.
            Backend::X86_64 => line.0 == 0,
            // RV32I cores take the machine external interrupt on line 0.
            // IA-32 exposes the maskable INTR request as line 0. The vector is
            // not part of the line on x86; the interrupt controller supplies it
            // through `queue_interrupt` (ADR 0018).
            Backend::X86Real | Backend::X86Protected | Backend::X86Boot => line.0 == 0,
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => line.0 == 0,
        };
        if !supported {
            return Err(EngineError::LineNotSupported);
        }
        self.irq_edges.push((line, high));
        Ok(())
    }
}

/// IA-32 architectural interrupt and exception entry.
///
/// Unicorn 2.1.x exposes no INTR/IRQ injection for any ISA, so the adapter
/// performs the sequence the hardware would perform (Intel SDM Vol.3 §6.12)
/// against guest memory. The implementation is deliberately literal, because
/// every step is observable by the guest's own handlers:
///
/// * a queued vector is only taken while `EFLAGS.IF` is set, and it waits in
///   request order otherwise;
/// * real mode reads the 4-byte table entry at `vector * 4` and pushes a
///   16-bit `FLAGS/CS/IP` frame on the `SS:SP` stack;
/// * protected mode decodes the 8-byte gate at `IDTR.base + vector * 8`,
///   rejects absent and task gates, pushes a frame whose width follows the
///   gate type, clears `IF`/`TF` for interrupt gates only, and loads the
///   handler selector through the CPU's own segment load;
/// * a vector that pushes an error code (Intel SDM Vol.3 Table 6-1) pushes it
///   below the return frame, where `iret` expects to find it;
/// * a ring change through a conforming gate is not modelled yet, and
///   long-mode (64-bit) gates are rejected rather than half-implemented.
///
/// Two callers share the path. [`Common::queue_interrupt`] uses it for external
/// interrupts at a quantum boundary, and the `UC_HOOK_INTR` hook uses it for
/// exceptions — because Unicorn 2.1.x reports a CPU exception to that hook and
/// then *discards* it: `cpu_handle_exception` (`accel/tcg/cpu-exec.c`) never
/// reaches the i386 `do_interrupt`, so without this a faulting guest would
/// resume on the faulting instruction with no handler entered (ADR 0020).
/// Everything the CPU computed before the hook is read out of the register
/// state: `handle_mmu_fault` (`target/i386/excp_helper.c`) writes `CR2` and the
/// error code itself, so both survive into the hook.
///
/// Instruction accounting is untouched: entry costs no guest instruction,
/// which matches the existing "budget = instructions executed" contract.
fn x86_reg(uc: &Unicorn<'_, EngineData>, reg: RegisterX86) -> Result<u64, EngineError> {
    uc.reg_read(reg as i32).map_err(|_| EngineError::Backend)
}

fn x86_read_u16(uc: &Unicorn<'_, EngineData>, address: u64) -> Result<u16, EngineError> {
    let mut buf = [0u8; 2];
    x86_read_bytes(uc, address, &mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

/// Read `buf` from the guest's *linear* `address`.
///
/// Unicorn 2.1.x's `uc_mem_read`/`uc_mem_write` are physical on i386:
/// `cpu_memory_rw_debug` ends in `address_space_read` and never consults the
/// guest's page tables. Every address this adapter reads on the guest's behalf
/// is linear — an `IDT` entry, a `GDT` descriptor, a stack slot, an
/// instruction — so the translation happens here, one page at a time, with the
/// same tables the CPU is walking.
fn x86_read_bytes(
    uc: &Unicorn<'_, EngineData>,
    address: u64,
    buf: &mut [u8],
) -> Result<(), EngineError> {
    let mut done = 0;
    while done < buf.len() {
        let at = address + done as u64;
        let physical = x86_translate(uc, at)?;
        let chunk = ((0x1000 - (at & 0xFFF)) as usize).min(buf.len() - done);
        uc.mem_read(physical, &mut buf[done..done + chunk])
            .map_err(|_| EngineError::Unmapped)?;
        done += chunk;
    }
    Ok(())
}

fn x86_write_bytes(
    uc: &mut Unicorn<'_, EngineData>,
    address: u64,
    buf: &[u8],
) -> Result<(), EngineError> {
    let mut done = 0;
    while done < buf.len() {
        let at = address + done as u64;
        let physical = x86_translate(uc, at)?;
        let chunk = ((0x1000 - (at & 0xFFF)) as usize).min(buf.len() - done);
        uc.mem_write(physical, &buf[done..done + chunk])
            .map_err(|_| EngineError::Unmapped)?;
        done += chunk;
    }
    // The frame lives in ordinary RAM, but a stale translation block that
    // cached the old stack contents must not survive the write.
    let end = address + buf.len() as u64;
    uc.ctl_remove_cache(address, end)
        .map_err(|_| EngineError::Backend)
}

/// Read straight from a *physical* address, for the tables a walk consults.
fn x86_read_physical(
    uc: &Unicorn<'_, EngineData>,
    address: u64,
    buf: &mut [u8],
) -> Result<(), EngineError> {
    uc.mem_read(address, buf).map_err(|_| EngineError::Unmapped)
}

/// Physical address backing the guest's linear `address`.
fn x86_translate(uc: &Unicorn<'_, EngineData>, address: u64) -> Result<u64, EngineError> {
    let (physical, present) = x86_page_walk(uc, address)?;
    if !present {
        return Err(EngineError::Unmapped);
    }
    Ok(physical)
}

/// One page-table walk, shared by the linear-to-physical translation above and
/// by the `P` bit of a `#PF` error code.
///
/// Returns the physical address the linear address resolves to and whether the
/// mapping is present. A walk that cannot read a table reports "present": an
/// unreadable table is not a missing page, and the protection-fault path is the
/// more informative answer. Paging off is an identity mapping.
fn x86_page_walk(uc: &Unicorn<'_, EngineData>, linear: u64) -> Result<(u64, bool), EngineError> {
    /// Intel SDM Vol.3 Fig.4-4 and 4-9: present, size, address.
    const CR0_PG: u64 = 1 << 31;
    const CR4_PAE: u64 = 1 << 5;
    const PRESENT: u64 = 1;
    const PAGE_SIZE: u64 = 1 << 7;
    const PAGE_4K: u64 = !0xFFF;
    /// PAE/64-bit entries carry a physical address in bits 51:12.
    const PAGE_PAE: u64 = 0x000F_FFFF_FFFF_F000;

    let linear = linear & 0xFFFF_FFFF;
    if x86_reg(uc, RegisterX86::CR0)? & CR0_PG == 0 {
        return Ok((linear, true));
    }
    let cr3 = x86_reg(uc, RegisterX86::CR3)?;
    if x86_reg(uc, RegisterX86::CR4)? & CR4_PAE == 0 {
        let mut raw = [0u8; 4];
        // 32-bit paging: one page directory, 4 KiB or 4 MiB pages.
        let at = (cr3 & PAGE_4K) + ((linear >> 22) & 0x3FF) * 4;
        x86_read_physical(uc, at, &mut raw).map_err(|_| EngineError::Unmapped)?;
        let pde = u64::from(u32::from_le_bytes(raw));
        if pde & PRESENT == 0 {
            return Ok((linear, false));
        }
        if pde & PAGE_SIZE != 0 {
            return Ok(((pde & 0xFFC0_0000) | (linear & 0x3F_FFFF), true));
        }
        let at = (pde & PAGE_4K) + ((linear >> 12) & 0x3FF) * 4;
        x86_read_physical(uc, at, &mut raw).map_err(|_| EngineError::Unmapped)?;
        let pte = u64::from(u32::from_le_bytes(raw));
        return Ok(((pte & PAGE_4K) | (linear & 0xFFF), pte & PRESENT != 0));
    }
    // PAE: page directory pointer table, page directory, page table.
    let mut raw = [0u8; 8];
    let at = (cr3 & !0x1F) + ((linear >> 30) & 3) * 8;
    x86_read_physical(uc, at, &mut raw).map_err(|_| EngineError::Unmapped)?;
    let pdpe = u64::from_le_bytes(raw);
    if pdpe & PRESENT == 0 {
        return Ok((linear, false));
    }
    let at = (pdpe & PAGE_PAE) + ((linear >> 21) & 0x1FF) * 8;
    x86_read_physical(uc, at, &mut raw).map_err(|_| EngineError::Unmapped)?;
    let pde = u64::from_le_bytes(raw);
    if pde & PRESENT == 0 {
        return Ok((linear, false));
    }
    if pde & PAGE_SIZE != 0 {
        return Ok(((pde & 0x000F_FFFF_FFE0_0000) | (linear & 0x1F_FFFF), true));
    }
    let at = (pde & PAGE_PAE) + ((linear >> 12) & 0x1FF) * 8;
    x86_read_physical(uc, at, &mut raw).map_err(|_| EngineError::Unmapped)?;
    let pte = u64::from_le_bytes(raw);
    Ok(((pte & PAGE_PAE) | (linear & 0xFFF), pte & PRESENT != 0))
}

/// Linear address of `offset` inside segment `selector`.
///
/// Real mode scales the selector by 16; everything else resolves the base
/// through the guest's own `GDT`, so a small-model guest is followed rather
/// than assumed flat (ADR 0018).
fn x86_linear(
    uc: &Unicorn<'_, EngineData>,
    selector: u16,
    offset: u64,
) -> Result<u64, EngineError> {
    if x86_reg(uc, RegisterX86::CR0)? & 1 == 0 {
        return Ok(u64::from(selector & 0xFFFC) * 16 + (offset & 0xFFFF));
    }
    Ok(x86_descriptor_base(uc, selector)? + offset)
}

/// Linear base of a segment selector's descriptor in the current GDT.
///
/// Flat-model guests (every catalog PC target) use base 0 descriptors, but the
/// value is read from the guest table rather than assumed, so a small-model
/// guest fails loudly instead of silently pushing its frame to the wrong
/// address. An unobserved `GDTR` is a capability gap, not a guess.
fn x86_descriptor_base(uc: &Unicorn<'_, EngineData>, selector: u16) -> Result<u64, EngineError> {
    if selector & 0x0004 != 0 {
        return Err(EngineError::CapabilityMissing);
    }
    let index = u64::from(selector >> 3);
    if index == 0 {
        return Ok(0);
    }
    let (base, limit) = uc
        .get_data()
        .tables
        .gdt
        .ok_or(EngineError::CapabilityMissing)?;
    let offset = index * 8;
    if offset + 7 > limit {
        return Err(EngineError::CapabilityMissing);
    }
    let mut raw = [0u8; 8];
    x86_read_bytes(uc, base + offset, &mut raw)?;
    // Intel SDM Vol.3 Fig.3-8: base is split across three fields.
    let segment_base = u64::from(raw[2])
        | (u64::from(raw[3]) << 8)
        | (u64::from(raw[4]) << 16)
        | (u64::from(raw[7]) << 24);
    Ok(segment_base & 0xFFFF_FFFF)
}

/// `IDTR` as `(base, limit)`, as observed at the guest's `LIDT`.
fn x86_idtr(uc: &Unicorn<'_, EngineData>) -> Result<(u64, u64), EngineError> {
    uc.get_data()
        .tables
        .idt
        .ok_or(EngineError::CapabilityMissing)
}

/// The return frame an interrupt entry will push.
struct X86Entry {
    /// `CS` the handler returns to.
    cs: u64,
    /// `EIP` the handler returns to: the faulting instruction for a CPU
    /// exception, the instruction *after* the `INT` for a software interrupt.
    eip: u64,
    /// Pushed below the return frame when the vector carries one.
    error_code: Option<u32>,
}

/// Enter `vector`'s gate and push `entry`'s frame. `Ok(false)` means the guest
/// has no usable gate for the vector (an all-zero real-mode slot, an absent
/// protected-mode descriptor), which the caller must not mistake for delivery.
fn x86_enter_gate(
    uc: &mut Unicorn<'_, EngineData>,
    vector: u8,
    entry: X86Entry,
) -> Result<bool, EngineError> {
    const IF: u64 = 1 << 9;
    const TF: u64 = 1 << 8;
    let flags = x86_reg(uc, RegisterX86::EFLAGS)?;
    let cr0 = x86_reg(uc, RegisterX86::CR0)?;
    let ss = u16::try_from(x86_reg(uc, RegisterX86::SS)?).unwrap_or(0);
    let esp = x86_reg(uc, RegisterX86::ESP)? & 0xFFFF_FFFF;

    if cr0 & 1 == 0 {
        let at = u64::from(vector) * 4;
        let handler_ip = u64::from(x86_read_u16(uc, at)?);
        let handler_cs = x86_read_u16(uc, at + 2)?;
        if handler_cs == 0 && handler_ip == 0 {
            // An empty IVT slot is memory, not a handler: entering `0:0` would
            // execute the interrupt table itself.
            return Ok(false);
        }
        let base = u64::from(ss) * 16;
        let mut sp = u32::from(esp as u16);
        // Real-mode frames are always 16-bit and never carry an error code.
        for value in [flags as u16, entry.cs as u16, entry.eip as u16] {
            sp = sp.wrapping_sub(2) & 0xFFFF;
            x86_write_bytes(uc, base + u64::from(sp), &value.to_le_bytes())?;
        }
        uc.reg_write(RegisterX86::ESP as i32, u64::from(sp))
            .map_err(|_| EngineError::Backend)?;
        // Real-mode interrupt entry clears IF and TF unconditionally.
        uc.reg_write(RegisterX86::EFLAGS as i32, flags & !(IF | TF))
            .map_err(|_| EngineError::Backend)?;
        uc.reg_write(RegisterX86::CS as i32, u64::from(handler_cs))
            .map_err(|_| EngineError::Backend)?;
        uc.reg_write(RegisterX86::EIP as i32, u64::from(handler_ip))
            .map_err(|_| EngineError::Backend)?;
        return Ok(true);
    }

    let (idtr_base, idtr_limit) = x86_idtr(uc)?;
    let gate_at = u64::from(vector) * 8;
    if gate_at + 7 > idtr_limit {
        return Err(EngineError::CapabilityMissing);
    }
    let mut raw = [0u8; 8];
    x86_read_bytes(uc, idtr_base + gate_at, &mut raw)?;
    let low = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    // Intel SDM Vol.3 Fig.6-2: byte 5 is P(1) DPL(2) S(1) Type(4).
    let present = raw[5] & 0x80 != 0;
    let gate_type = raw[5] & 0x0F;
    if !present {
        uc.get_data_mut().last_error = Some(format!(
            "gate {vector:#04x} absent: {raw:02x?} at {:#x}",
            idtr_base + gate_at
        ));
        return Ok(false);
    }
    if gate_type == 0x5 {
        // Task gates switch the whole task; out of scope for this slice.
        return Err(EngineError::CapabilityMissing);
    }
    let selector = (low >> 16) as u16;
    let offset_low = low & 0xFFFF;
    let offset_high = u32::from(u16::from_le_bytes([raw[6], raw[7]]));
    let handler = u64::from(offset_low) | (u64::from(offset_high) << 16);
    let wide = matches!(gate_type, 0xE | 0xF);
    let ss_base = if ss == 0 {
        0
    } else {
        x86_descriptor_base(uc, ss)?
    };
    let mut sp = esp;
    if wide {
        for value in [flags as u32, u32::from(entry.cs as u16), entry.eip as u32] {
            sp = sp.wrapping_sub(4);
            x86_write_bytes(uc, ss_base + u64::from(sp), &value.to_le_bytes())?;
        }
    } else {
        for value in [flags as u16, entry.cs as u16, entry.eip as u16] {
            sp = sp.wrapping_sub(2);
            x86_write_bytes(uc, ss_base + u64::from(sp), &value.to_le_bytes())?;
        }
    }
    if let Some(error_code) = entry.error_code {
        // Intel SDM Vol.3 §6.13: the error code is pushed last, so it sits at
        // the top of the frame and `iret` pops past it.
        sp = sp.wrapping_sub(4);
        x86_write_bytes(uc, ss_base + u64::from(sp), &error_code.to_le_bytes())?;
    }
    uc.reg_write(RegisterX86::ESP as i32, u64::from(sp))
        .map_err(|_| EngineError::Backend)?;
    if x86_gate_clears_if(gate_type) {
        uc.reg_write(RegisterX86::EFLAGS as i32, flags & !(IF | TF))
            .map_err(|_| EngineError::Backend)?;
    }
    uc.reg_write(RegisterX86::CS as i32, u64::from(selector))
        .map_err(|_| EngineError::Backend)?;
    uc.reg_write(RegisterX86::EIP as i32, handler)
        .map_err(|_| EngineError::Backend)?;
    Ok(true)
}

/// Vectors that push an error code in real hardware (Intel SDM Vol.3 Table
/// 6-1). The list is literal so a guest handler and its `iret` agree on the
/// frame width; `#DF` is on it even though this adapter cannot enter a
/// double-fault handler usefully.
const fn x86_vector_pushes_error_code(vector: u8) -> bool {
    matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
}

/// Length of the software-interrupt instruction at `CS:EIP`, or `None` when
/// the byte there is not one.
///
/// Unicorn reports a software `INT n` through the same `UC_HOOK_INTR` hook as a
/// CPU exception. This build folds `EIP` past the `INT` before the hook runs
/// (verified on 2.1.5), which `Common::x86_deliver_exception` detects through
/// the recorded instruction address; the encoding check here covers the other
/// case, so the frame is correct for either behaviour.
fn x86_software_interrupt_length(uc: &Unicorn<'_, EngineData>, cs: u16, eip: u64) -> Option<u64> {
    let linear = x86_linear(uc, cs, eip).ok()?;
    let mut opcode = [0u8; 1];
    x86_read_bytes(uc, linear, &mut opcode).ok()?;
    match opcode[0] {
        // INT3 (`CC`), INTO (`CE`) and INT imm8 (`CD ib`); `F1` (ICEBP) raises
        // #DB instead of entering a gate, so it is not in this list.
        0xCC | 0xCE => Some(1),
        0xCD => Some(2),
        _ => None,
    }
}

/// Deliver a vector the interrupt service declined to the guest's own gate.
///
/// `Ok(true)` means a handler was entered and the quantum continues inside it;
/// `Ok(false)` means the guest has no gate to take the vector through.
fn x86_deliver_exception(
    uc: &mut Unicorn<'_, EngineData>,
    vector: u8,
) -> Result<bool, EngineError> {
    let cs = u16::try_from(x86_reg(uc, RegisterX86::CS)?).unwrap_or(0);
    let eip = x86_reg(uc, RegisterX86::EIP)? & 0xFFFF_FFFF;
    // A software `INT` has already had its return address folded; an exception
    // has not, and `EIP` still names the instruction that faulted.
    let linear = x86_linear(uc, cs, eip).ok();
    let executing = uc.get_data().executing;
    let folded = executing.is_some() && executing != Some(eip) && executing != linear;
    let (return_eip, error_code) = if folded {
        (eip, None)
    } else if let Some(length) = x86_software_interrupt_length(uc, cs, eip) {
        (eip + length, None)
    } else {
        (
            eip,
            x86_vector_pushes_error_code(vector).then(|| x86_error_code(uc, vector, cs, eip)),
        )
    };
    // Keep the fault for the report: the vector alone does not say where it
    // happened, and `CR2` is the only record of which address the guest wanted.
    let cr2 = x86_reg(uc, RegisterX86::CR2).unwrap_or(0);
    uc.get_data_mut().exception = Some((vector, eip, cr2));
    x86_enter_gate(
        uc,
        vector,
        X86Entry {
            cs: u64::from(cs),
            eip: return_eip,
            error_code,
        },
    )
}

/// The error code the CPU would have pushed for `vector`.
///
/// Unicorn computes the real one — `handle_mmu_fault` writes `env->error_code`
/// and `CR2` immediately before raising `#PF` — but exposes neither through any
/// register or hook, so the adapter reconstructs it from what it *can* see:
/// `CR2` names the faulting address, the guest's page tables answer the present
/// bit, and the faulting instruction's opcode answers read-versus-write. Only
/// `#PF` carries information this way; the other error-code vectors push zero,
/// which is what `#DE` and (absent a selector) `#GP` push anyway.
fn x86_error_code(uc: &Unicorn<'_, EngineData>, vector: u8, cs: u16, eip: u64) -> u32 {
    /// Intel SDM Vol.3 Table 6-2: present, write, user, reserved, I/D.
    const PRESENT: u32 = 1 << 0;
    const WRITE: u32 = 1 << 1;
    const USER: u32 = 1 << 2;
    const INSTRUCTION: u32 = 1 << 4;
    const CR4_SMEP: u64 = 1 << 20;

    if vector != 14 {
        return 0;
    }
    let Ok(cr2) = x86_reg(uc, RegisterX86::CR2) else {
        return 0;
    };
    let mut code = 0;
    if x86_page_present(uc, cr2) {
        code |= PRESENT;
    }
    let fetch = x86_linear(uc, cs, eip).map(|at| at == cr2).unwrap_or(false);
    if fetch {
        // Without PAE there is no `EFER.NXE`, so only SMEP makes a fetch fault
        // distinguishable from a data access in the error code.
        if x86_reg(uc, RegisterX86::CR4).map(|cr4| cr4 & CR4_SMEP != 0) == Ok(true) {
            code |= INSTRUCTION;
        }
    } else if x86_faulting_access_is_write(uc, cs, eip) {
        code |= WRITE;
    }
    if cs & 3 == 3 {
        code |= USER;
    }
    code
}

/// Whether the guest's own page tables map `linear` — the present bit of a
/// `#PF` error code. A walk that cannot read a table reports "present", because
/// an unreadable table means the fault was not a plain missing page and the
/// protection path is the more informative answer.
fn x86_page_present(uc: &Unicorn<'_, EngineData>, linear: u64) -> bool {
    x86_page_walk(uc, linear)
        .map(|(_, present)| present)
        .unwrap_or(true)
}

/// Whether the instruction at `CS:EIP` stores to memory.
///
/// `#PF`'s write bit is the one bit the adapter cannot read back from anywhere,
/// so it is classified from the opcode byte. The classification only steers a
/// diagnostic bit and the kernel's read-only retry path: it can never change
/// which instruction the guest resumes on, because the return address is the
/// faulting instruction either way. Unknown encodings are reported as reads,
/// which is the safe direction — a spurious "read fault" is retried, a spurious
/// "write fault" would be treated as a protection violation.
fn x86_faulting_access_is_write(uc: &Unicorn<'_, EngineData>, cs: u16, eip: u64) -> bool {
    let Ok(linear) = x86_linear(uc, cs, eip) else {
        return false;
    };
    let mut code = [0u8; 16];
    if x86_read_bytes(uc, linear, &mut code).is_err() {
        return false;
    }
    x86_stores_to_memory(&code)
}

/// Classify one instruction encoding: `true` when its destination is memory.
///
/// The tables below cover the general-purpose store forms — including the
/// `ModRM.reg`-selected groups — plus the SSE stores. Everything else, from
/// arithmetic on registers to string loads, reports a read.
fn x86_stores_to_memory(code: &[u8]) -> bool {
    // Legacy prefixes only: `40..=4F` are `INC`/`DEC` outside long mode.
    let mut at = 0;
    while matches!(
        code.get(at),
        Some(0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3)
    ) {
        at += 1;
    }
    let reg_field = |offset: usize| code.get(offset).map(|byte| (byte >> 3) & 7);
    match code.get(at) {
        // `r/m <- r` forms of the ALU and move groups.
        Some(
            0x00 | 0x01 | 0x08 | 0x09 | 0x10 | 0x11 | 0x18 | 0x19 | 0x20 | 0x21 | 0x28 | 0x29
            | 0x30 | 0x31 | 0x38 | 0x39,
        ) => true,
        // `ARPL r/m16, r16`.
        Some(0x63) => true,
        Some(0x0F) => {
            let second = code.get(at + 1);
            match second {
                // BTS/BTR/BTC with a register source, CMPXCHG8B, and the SSE
                // stores (`MOVLPS`/`MOVHPS`/`MOVNTPS`/`MOVD`-family).
                Some(0xAB | 0xB3 | 0xBB | 0x11 | 0x13 | 0x17 | 0x29 | 0x2B | 0x7F | 0xD6) => true,
                Some(0xBA) => matches!(reg_field(at + 2), Some(5 | 6 | 7)),
                Some(0xC7) => matches!(reg_field(at + 2), Some(1)),
                _ => false,
            }
        }
        // ALU `r/m, imm`, `XCHG`, `MOV r/m, r`, `POP r/m`, the shift groups and
        // the byte-string stores.
        Some(
            0x80 | 0x81 | 0x82 | 0x83 | 0x86 | 0x87 | 0x88 | 0x89 | 0x8F | 0xA2 | 0xA3 | 0xA4
            | 0xA5 | 0xAA | 0xAB | 0xC0 | 0xC1 | 0xC6 | 0xC7 | 0xD0 | 0xD1 | 0xD2 | 0xD3,
        ) => true,
        // `NOT`/`NEG` are the only storing members of group 3.
        Some(0xF6 | 0xF7) => matches!(reg_field(at + 1), Some(2 | 3)),
        // `INC`/`DEC r/m` store; the calls and jumps in group 5 only read.
        Some(0xFE | 0xFF) => matches!(reg_field(at + 1), Some(0 | 1)),
        _ => false,
    }
}

/// Take a vector the interrupt service declined, or stop the quantum when the
/// guest has nowhere to take it.
fn x86_take_exception(uc: &mut Unicorn<'_, EngineData>, vector: u8) {
    let delivered = match x86_deliver_exception(uc, vector) {
        Ok(delivered) => delivered,
        Err(error) => {
            // The gate table itself refused the vector: that is a diagnosis,
            // not just a stop, so it goes next to the backend's own words.
            uc.get_data_mut().last_error =
                Some(format!("exception {vector:#04x} undeliverable: {error:?}"));
            false
        }
    };
    if delivered {
        return;
    }
    // Nothing claimed the vector and the guest has no gate for it: stop so the
    // caller decides instead of silently skipping the fault.
    uc.get_data_mut().unclaimed = Some(vector);
    let _ = uc.emu_stop();
}

impl Common {
    /// Deliver one pending vector if the guest can take it. `Ok(false)` means
    /// interrupts are disabled and the vector stays queued.
    fn x86_deliver_one(&mut self, vector: u8) -> Result<bool, EngineError> {
        let flags = x86_reg(&self.inner.0, RegisterX86::EFLAGS)? as u32;
        // IF gates INTR only; the queued request must survive until it is set.
        const IF: u32 = 1 << 9;
        if flags & IF == 0 {
            return Ok(false);
        }
        let cs = u16::try_from(x86_reg(&self.inner.0, RegisterX86::CS)?).unwrap_or(0);
        let eip = x86_reg(&self.inner.0, RegisterX86::EIP)? & 0xFFFF_FFFF;
        x86_enter_gate(
            &mut self.inner.0,
            vector,
            X86Entry {
                cs: u64::from(cs),
                eip,
                error_code: None,
            },
        )
    }

    /// Drain queued vectors in request order until one cannot be taken.
    fn x86_deliver_pending(&mut self) -> Result<(), EngineError> {
        while let Some(vector) = self.pending_interrupts.front().copied() {
            if !self.x86_deliver_one(vector)? {
                break;
            }
            self.pending_interrupts.pop_front();
        }
        Ok(())
    }
}

/// Decode `LGDT`/`LIDT` (`0F 01 /2` and `0F 01 /3`) at `address` and record the
/// pseudo-descriptor it names.
///
/// Only the memory forms are meaningful here: the register forms do not exist
/// for these opcodes. The operand's linear address assumes a flat `DS`, which
/// holds for every catalog PC target and is checked implicitly by the subsequent
/// guest reads: a wrong base produces an unreadable descriptor and the vector
/// stays undelivered rather than landing somewhere plausible.
fn x86_observe_descriptor_load(uc: &mut Unicorn<'_, EngineData>, address: u64) {
    let mut opcode = [0u8; 3];
    if x86_read_bytes(uc, address, &mut opcode).is_err() {
        return;
    }
    if opcode[0] != 0x0F || opcode[1] != 0x01 {
        return;
    }
    let modrm = opcode[2];
    let reg = (modrm >> 3) & 0x07;
    if reg != 2 && reg != 3 {
        return;
    }
    let real_mode = uc
        .reg_read(RegisterX86::CR0 as i32)
        .map(|cr0| cr0 & 1 == 0)
        .unwrap_or(true);
    let Some(operand) = x86_modrm_operand(uc, address + 2, modrm, real_mode) else {
        return;
    };
    let mut descriptor = [0u8; 6];
    if x86_read_bytes(uc, operand, &mut descriptor).is_err() {
        return;
    }
    let limit = u64::from(u16::from_le_bytes([descriptor[0], descriptor[1]]));
    let base = u64::from(u32::from_le_bytes([
        descriptor[2],
        descriptor[3],
        descriptor[4],
        descriptor[5],
    ]));
    let data = uc.get_data_mut();
    if reg == 2 {
        data.tables.gdt = Some((base, limit));
    } else {
        data.tables.idt = Some((base, limit));
    }
}

/// Linear address of a ModRM memory operand whose ModRM byte is at
/// `modrm_at`, interpreted with the guest's current address size.
///
/// Real mode (and any 16-bit address size) uses the two-operand table of Intel
/// SDM Vol.2 Table 2-1; everything else uses the SIB form. The segment base is
/// added from the segment register's selector, which is exact in real mode
/// where the base is `selector * 16`. Returns `None` for register forms and
/// encodings this slice does not model, so a caller simply skips the load.
fn x86_modrm_operand(
    uc: &Unicorn<'_, EngineData>,
    modrm_at: u64,
    modrm: u8,
    real_mode: bool,
) -> Option<u64> {
    let mode = modrm >> 6;
    let rm = modrm & 0x07;
    if mode == 0b11 {
        return None;
    }
    if real_mode {
        let at = modrm_at + 1;
        let ds = u64::from(uc.reg_read(RegisterX86::DS as i32).ok()? as u16) * 16;
        if mode == 0b00 && rm == 0b110 {
            let mut disp = [0u8; 2];
            x86_read_bytes(uc, at, &mut disp).ok()?;
            return Some(ds + u64::from(u16::from_le_bytes(disp)));
        }
        let mut base = 0u64;
        let bx = || uc.reg_read(RegisterX86::EBX as i32).ok();
        let bp = || uc.reg_read(RegisterX86::EBP as i32).ok();
        let si = || uc.reg_read(RegisterX86::ESI as i32).ok();
        let di = || uc.reg_read(RegisterX86::EDI as i32).ok();
        let (low, uses_bp) = match rm {
            0 => (bx()? as u16 as u64 + si()? as u16 as u64, false),
            1 => (bx()? as u16 as u64 + di()? as u16 as u64, false),
            2 => (bp()? as u16 as u64 + si()? as u16 as u64, true),
            3 => (bp()? as u16 as u64 + di()? as u16 as u64, true),
            4 => (si()? as u16 as u64, false),
            5 => (di()? as u16 as u64, false),
            6 => (bp()? as u16 as u64, true),
            _ => (bx()? as u16 as u64, false),
        };
        base += low;
        // BP-based operands default to SS; every other form defaults to DS.
        let selector = if uses_bp {
            uc.reg_read(RegisterX86::SS as i32).ok()? as u16
        } else {
            uc.reg_read(RegisterX86::DS as i32).ok()? as u16
        };
        base += u64::from(selector) * 16;
        match mode {
            0b00 => {}
            0b01 => {
                let mut disp = [0u8; 1];
                x86_read_bytes(uc, at, &mut disp).ok()?;
                base = base.wrapping_add(disp[0] as i8 as i64 as u64);
            }
            _ => {
                let mut disp = [0u8; 2];
                x86_read_bytes(uc, at, &mut disp).ok()?;
                base = base.wrapping_add(u64::from(u16::from_le_bytes(disp)));
            }
        }
        return Some(base & 0x10_FFEF);
    }
    // Every path below assigns `offset` before it is read.
    let mut offset;
    let mut at = modrm_at + 1;
    if rm == 0b101 && mode == 0b00 {
        let mut disp = [0u8; 4];
        x86_read_bytes(uc, at, &mut disp).ok()?;
        return Some(u64::from(u32::from_le_bytes(disp)));
    }
    if rm == 0b100 {
        let mut sib = [0u8; 1];
        x86_read_bytes(uc, at, &mut sib).ok()?;
        at += 1;
        let base_field = sib[0] & 0x07;
        let base = match base_field {
            0 => uc.reg_read(RegisterX86::EAX as i32).ok()?,
            1 => uc.reg_read(RegisterX86::ECX as i32).ok()?,
            2 => uc.reg_read(RegisterX86::EDX as i32).ok()?,
            3 => uc.reg_read(RegisterX86::EBX as i32).ok()?,
            4 => uc.reg_read(RegisterX86::ESP as i32).ok()?,
            5 => {
                if mode == 0b00 {
                    0
                } else {
                    uc.reg_read(RegisterX86::EBP as i32).ok()?
                }
            }
            6 => uc.reg_read(RegisterX86::ESI as i32).ok()?,
            _ => uc.reg_read(RegisterX86::EDI as i32).ok()?,
        };
        let index_field = (sib[0] >> 3) & 0x07;
        let index = if index_field == 0b100 {
            0
        } else {
            match index_field {
                0 => uc.reg_read(RegisterX86::EAX as i32).ok()?,
                1 => uc.reg_read(RegisterX86::ECX as i32).ok()?,
                2 => uc.reg_read(RegisterX86::EDX as i32).ok()?,
                3 => uc.reg_read(RegisterX86::EBX as i32).ok()?,
                5 => uc.reg_read(RegisterX86::EBP as i32).ok()?,
                6 => uc.reg_read(RegisterX86::ESI as i32).ok()?,
                _ => uc.reg_read(RegisterX86::EDI as i32).ok()?,
            }
        };
        let shift = sib[0] >> 6;
        offset = base
            & 0xFFFF_FFFF
                + ((index & 0xFFFF_FFFF) << shift)
                + if base_field == 0b101 && mode == 0b00 {
                    let mut disp = [0u8; 4];
                    x86_read_bytes(uc, at, &mut disp).ok()?;
                    at += 4;
                    u64::from(u32::from_le_bytes(disp))
                } else {
                    0
                };
        offset &= 0xFFFF_FFFF;
    } else {
        let base = match rm {
            0 => uc.reg_read(RegisterX86::EAX as i32).ok()?,
            1 => uc.reg_read(RegisterX86::ECX as i32).ok()?,
            2 => uc.reg_read(RegisterX86::EDX as i32).ok()?,
            3 => uc.reg_read(RegisterX86::EBX as i32).ok()?,
            5 => uc.reg_read(RegisterX86::EBP as i32).ok()?,
            6 => uc.reg_read(RegisterX86::ESI as i32).ok()?,
            _ => uc.reg_read(RegisterX86::EDI as i32).ok()?,
        };
        offset = base & 0xFFFF_FFFF;
    }
    match mode {
        0b00 => {}
        0b01 => {
            let mut disp = [0u8; 1];
            x86_read_bytes(uc, at, &mut disp).ok()?;
            offset = offset.wrapping_add(disp[0] as i8 as i64 as u64);
        }
        _ => {
            let mut disp = [0u8; 4];
            x86_read_bytes(uc, at, &mut disp).ok()?;
            offset = offset.wrapping_add(u64::from(u32::from_le_bytes(disp)));
        }
    }
    Some(offset & 0xFFFF_FFFF)
}

/// Whether the instruction at `address` is a repeat-prefixed string
/// instruction, whose iterations all share one `EIP`.
///
/// A `rep movs` of a few megabytes is one instruction and millions of budget
/// slots, and Unicorn cannot resume a context stopped in the middle of one, so
/// the quantum has to let it retire.
fn x86_is_rep_string(uc: &Unicorn<'_, EngineData>, address: u64) -> bool {
    let mut raw = [0u8; 6];
    if x86_read_bytes(uc, address, &mut raw).is_err() {
        return false;
    }
    let mut index = 0;
    while index < raw.len() {
        match raw[index] {
            // Operand/address-size and segment prefixes.
            0x66 | 0x67 | 0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 => index += 1,
            // `rep`/`repe`/`repne`.
            0xF3 | 0xF2 => {
                return matches!(
                    raw.get(index + 1),
                    Some(0xA4 | 0xA5 | 0xA6 | 0xA7 | 0xAA | 0xAB | 0xAC | 0xAD | 0xAE | 0xAF)
                );
            }
            _ => return false,
        }
    }
    false
}

/// Hardware clears `IF` on entry through an interrupt gate only
/// (Intel SDM Vol.3 Table 6-2). Kept here so the adapter does not depend on
/// the ISA vocabulary crate at runtime, matching the register-map convention.
const fn x86_gate_clears_if(gate_type: u8) -> bool {
    matches!(gate_type, 0x6 | 0xE)
}

fn prot_of(permissions: Permissions) -> Prot {
    let mut prot = Prot::NONE;
    if permissions.read {
        prot |= Prot::READ;
    }
    if permissions.write {
        prot |= Prot::WRITE;
    }
    if permissions.execute {
        prot |= Prot::EXEC;
    }
    prot
}

/// Generates one public per-ISA adapter struct over [`Common`]: the inherent
/// composition API plus the engine-neutral [`CpuEngine`] implementation.
macro_rules! unicorn_wrapper {
    ($(#[$meta:meta])* $name:ident => $backend:expr) => {
        $(#[$meta])*
        pub struct $name(Common);

        impl $name {
            pub fn new() -> Result<Self, EngineError> {
                Ok(Self(Common::new($backend)?))
            }

            pub fn attach_bus(&mut self, bus: Box<dyn EngineBus>) -> Result<(), EngineError> {
                self.0.attach_bus(bus)
            }

            pub fn map_mmio(&mut self, base: Addr, size: u64) -> Result<(), EngineError> {
                self.0.map_mmio(base, size)
            }

            pub fn map_shared_memory(
                &mut self,
                base: Addr,
                size: u64,
                permissions: Permissions,
                ram: Arc<SharedRam>,
            ) -> Result<(), EngineError> {
                self.0.map_shared_memory(base, size, permissions, ram)
            }

            /// Copy the authoritative shared buffers into this engine's
            /// mirrors (before a core slice).
            pub fn sync_shared_in(&mut self) -> Result<(), EngineError> {
                self.0.sync_shared_in()
            }

            /// Copy this engine's shared mirrors back into the
            /// authoritative buffers (after a core slice).
            pub fn sync_shared_out(&mut self) -> Result<(), EngineError> {
                self.0.sync_shared_out()
            }

            pub fn take_bus_fault(&mut self) -> Option<EngineBusFault> {
                self.0.take_bus_fault()
            }

            pub fn irq_edges(&self) -> &[(IrqLine, bool)] {
                self.0.irq_edges()
            }

            pub fn irq_level(&self, line: IrqLine) -> Option<bool> {
                self.0.irq_level(line)
            }

            pub fn save_context(&self) -> Result<UnicornContext, EngineError> {
                self.0.save_context()
            }

            pub fn restore_context(&mut self, context: &UnicornContext) -> Result<(), EngineError> {
                self.0.restore_context(context)
            }
        }

        impl CpuEngine for $name {
            fn caps(&self) -> EngineCaps {
                EngineCaps::COUNTED
            }

            fn run(&mut self, instructions: NonZeroU64) -> RunOutcome {
                self.0.run(instructions)
            }

            fn reg_read(&self, reg: RegId) -> Result<u64, EngineError> {
                self.0.reg_read(reg)
            }

            fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError> {
                self.0.reg_write(reg, value)
            }

            fn map_memory(
                &mut self,
                base: Addr,
                size: u64,
                permissions: Permissions,
            ) -> Result<(), EngineError> {
                self.0.map_memory(base, size, permissions)
            }

            fn write_memory(&mut self, base: Addr, data: &[u8]) -> Result<(), EngineError> {
                self.0.write_memory(base, data)
            }

            fn read_memory(&self, base: Addr, data: &mut [u8]) -> Result<(), EngineError> {
                self.0.read_memory(base, data)
            }

            fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError> {
                self.0.set_line(line, high)
            }

            fn take_bus_fault(&mut self) -> Option<EngineBusFault> {
                self.0.take_bus_fault()
            }

            fn line_edges(&self) -> &[(IrqLine, bool)] {
                self.0.irq_edges()
            }

            fn sync_shared_in(&mut self) -> Result<(), EngineError> {
                self.0.sync_shared_in()
            }

            fn sync_shared_out(&mut self) -> Result<(), EngineError> {
                self.0.sync_shared_out()
            }
        }
    };
}

unicorn_wrapper! {
    /// ARM A32 little-endian core adapter (ARM926EJ-S class).
    UnicornArm => Backend::Arm
}

unicorn_wrapper! {
    /// ARMv7-M little-endian core adapter (Cortex-M3 class). Constructed
    /// with `UC_MODE_MCLASS`, so the model executes T32 from reset and PC
    /// writes use halfword alignment (ADR 0012). Intended for M-profile
    /// board slices such as STM32F103.
    UnicornCortexM3 => Backend::ArmMClass
}

unicorn_wrapper! {
    /// AArch64 (ARM 64-bit) little-endian core adapter.
    UnicornArm64 => Backend::Arm64
}

unicorn_wrapper! {
    /// x86_64 64-bit core adapter.
    UnicornX86_64 => Backend::X86_64
}

unicorn_wrapper! {
    /// IA-32 adapter that starts in real mode (`CR0.PE = 0`), the state a PC
    /// boots in. Use [`UnicornX86::protected_mode`] to construct a core whose
    /// Unicorn mode word is 32-bit from the first instruction, for guests that
    /// enter protected mode before the first emulated instruction.
    UnicornX86 => Backend::X86Real
}

unicorn_wrapper! {
    /// IA-32 that starts in real mode but is *resumable* across the transition
    /// into protected mode: a PC-class boot core (ADR 0020).
    UnicornX86Boot => Backend::X86Boot
}

impl UnicornX86 {
    /// IA-32 with a 32-bit default operand size. `CR0.PE` is still guest
    /// owned: the guest's `CR0` write is what switches the decoder to
    /// protected-mode semantics, so only the initial mode word differs.
    pub fn protected_mode() -> Result<Self, EngineError> {
        Ok(Self(Common::new(Backend::X86Protected)?))
    }
}

macro_rules! x86_extensions {
    ($name:ident) => {
        impl $name {
            /// Install the `IN`/`OUT` port. One port serves both directions.
            pub fn attach_ports(&mut self, ports: Box<dyn PortBus>) -> Result<(), EngineError> {
                self.0.attach_ports(ports)
            }

            /// Request delivery of `vector`. The request is taken at the start of the
            /// next [`CpuEngine::run`] quantum, and only while `EFLAGS.IF` is set.
            pub fn queue_interrupt(&mut self, vector: u8) {
                self.0.pending_interrupts.push_back(vector);
            }

            /// Vectors still waiting for `EFLAGS.IF` to open.
            pub fn pending_interrupts(&self) -> usize {
                self.0.pending_interrupts.len()
            }

            /// `IDTR` as `(base, limit)`; the composition root uses it for diagnostics,
            /// the adapter itself re-reads it at delivery time.
            pub fn idtr(&self) -> Result<(u64, u64), EngineError> {
                x86_idtr(&self.0.inner.0)
            }

            /// `GDTR` descriptor base for `selector`, resolved through the guest GDT.
            pub fn segment_base(&self, selector: u16) -> Result<u64, EngineError> {
                x86_descriptor_base(&self.0.inner.0, selector)
            }

            /// Record the last `capacity` instruction addresses, retrievable with
            /// [`UnicornX86::instruction_trace`]. Off by default: the hook doubles the
            /// cost of every instruction.
            pub fn trace_instructions(&mut self, capacity: usize) -> Result<(), EngineError> {
                let data = self.0.inner.0.get_data_mut();
                data.trace = Some(std::collections::VecDeque::with_capacity(capacity));
                let capacity = capacity.max(1);
                self.0
                    .inner
                    .0
                    .add_code_hook(1, 0, move |uc, address, size| {
                        // `CS` is read through the vendor binding, which is the only
                        // place that knows the identifier.
                        let cs = uc.reg_read(RegisterX86::CS as i32).unwrap_or(0);
                        let data = uc.get_data_mut();
                        if let Some(trace) = data.trace.as_mut() {
                            if trace.len() == capacity {
                                trace.pop_front();
                            }
                            trace.push_back((address, size as u32, cs));
                        }
                    })
                    .map_err(|_| EngineError::Backend)?;
                Ok(())
            }

            /// The backend's own words for why the last quantum stopped.
            pub fn last_error(&self) -> Option<String> {
                self.0.inner.0.get_data().last_error.clone()
            }

            /// The vector neither the interrupt service nor the guest's gate
            /// table could take since the previous call, if any. Cleared on
            /// read, like the unmapped access below.
            pub fn take_unclaimed_interrupt(&mut self) -> Option<u8> {
                self.0.inner.0.get_data_mut().unclaimed.take()
            }

            /// The last CPU exception this adapter delivered through the
            /// guest's gate table, as `(vector, eip, cr2)` — the faulting
            /// instruction and, for `#PF`, the address the guest wanted.
            /// Cleared on read.
            pub fn take_exception(&mut self) -> Option<(u8, u64, u64)> {
                self.0.inner.0.get_data_mut().exception.take()
            }

            /// The first unmapped access since the previous call, as
            /// `(is_write, address, size)`. Cleared so the next quantum reports its own
            /// fault rather than the one already diagnosed.
            pub fn take_unmapped(&mut self) -> Option<(bool, u64, usize)> {
                self.0.inner.0.get_data_mut().unmapped.take()
            }

            /// Record every store into `[base, base + length)`, with the address of the
            /// instruction that made it. This is how an overwritten code page is
            /// traced back to the writer instead of guessed at.
            pub fn watch_writes(&mut self, base: u64, length: u64) -> Result<(), EngineError> {
                self.0.inner.0.get_data_mut().watch.push((base, length));
                self.0
                    .inner
                    .0
                    .add_mem_hook(
                        HookType::MEM_WRITE,
                        0,
                        u64::MAX,
                        move |uc, _, address, size, _| {
                            let data = uc.get_data();
                            let watched = data.watch.iter().any(|(start, length)| {
                                address >= *start && address < start + length
                            });
                            if !watched {
                                return true;
                            }
                            let pc = uc.reg_read(RegisterX86::EIP as i32).unwrap_or(0);
                            let data = uc.get_data_mut();
                            if data.writes.len() < 4096 {
                                data.writes.push((address, pc, size as u32));
                            }
                            true
                        },
                    )
                    .map_err(|_| EngineError::Backend)?;
                Ok(())
            }

            /// Stores seen in the watched regions, oldest first.
            pub fn watched_writes(&self) -> Vec<(u64, u64, u32)> {
                self.0.inner.0.get_data().writes.clone()
            }

            /// The recorded instructions as `(address, size, cs)`, oldest first.
            pub fn instruction_trace(&self) -> Vec<(u64, u32, u64)> {
                self.0
                    .inner
                    .0
                    .get_data()
                    .trace
                    .as_ref()
                    .map(|trace| trace.iter().copied().collect())
                    .unwrap_or_default()
            }

            /// Answer software interrupts through `service`, as firmware would.
            pub fn attach_interrupt_service(
                &mut self,
                service: Box<dyn InterruptService>,
            ) -> Result<(), EngineError> {
                let data = self.0.inner.0.get_data_mut();
                if data.interrupts.is_some() {
                    return Err(EngineError::BusAlreadyAttached);
                }
                data.interrupts = Some(service);
                Ok(())
            }
        }
    };
}

x86_extensions!(UnicornX86);
x86_extensions!(UnicornX86Boot);

/// [`SoftwareInterruptCpu`] view over the live Unicorn handle.
///
/// It only lives for the duration of one service callback; the register map is
/// the backend's, so a service sees exactly what guest code would see.
struct InterruptCpuView<'a, 'b> {
    uc: &'a mut Unicorn<'b, EngineData>,
}

impl InterruptCpuView<'_, '_> {
    fn register(&self, reg: RegId) -> Result<i32, EngineError> {
        match reg.0 {
            0 => Ok(RegisterX86::EAX as i32),
            1 => Ok(RegisterX86::ECX as i32),
            2 => Ok(RegisterX86::EDX as i32),
            3 => Ok(RegisterX86::EBX as i32),
            4 => Ok(RegisterX86::ESP as i32),
            5 => Ok(RegisterX86::EBP as i32),
            6 => Ok(RegisterX86::ESI as i32),
            7 => Ok(RegisterX86::EDI as i32),
            8 => Ok(RegisterX86::EIP as i32),
            9 => Ok(RegisterX86::EFLAGS as i32),
            10 => Ok(RegisterX86::CS as i32),
            11 => Ok(RegisterX86::DS as i32),
            12 => Ok(RegisterX86::ES as i32),
            13 => Ok(RegisterX86::FS as i32),
            14 => Ok(RegisterX86::GS as i32),
            15 => Ok(RegisterX86::SS as i32),
            _ => Err(EngineError::InvalidRegister),
        }
    }
}

impl SoftwareInterruptCpu for InterruptCpuView<'_, '_> {
    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError> {
        self.uc
            .reg_read(self.register(reg)?)
            .map_err(|_| EngineError::Backend)
    }

    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError> {
        let id = self.register(reg)?;
        self.uc
            .reg_write(id, value)
            .map_err(|_| EngineError::Backend)
    }

    fn mem_read(&self, address: u64, data: &mut [u8]) -> Result<(), EngineError> {
        self.uc
            .mem_read(address, data)
            .map_err(|_| EngineError::Backend)
    }

    fn mem_write(&mut self, address: u64, data: &[u8]) -> Result<(), EngineError> {
        self.uc
            .mem_write(address, data)
            .map_err(|_| EngineError::Backend)
    }
}

#[cfg(feature = "arch-riscv")]
unicorn_wrapper! {
    /// RISC-V RV32I little-endian core adapter (SiFive E31 class cores run
    /// the base integer subset; the compressed extension is unused by the
    /// catalog firmware, and PC writes must be 4-byte aligned).
    UnicornRiscV32 => Backend::RiscV32
}

/// MMIO read at an absolute guest address. All state lives in the engine-owned
/// [`EngineData`], so no host reference can escape through the callback.
fn mmio_read_at(uc: &mut Unicorn<'_, EngineData>, address: u64, size: usize) -> u64 {
    let Ok(width) = AccessSize::try_from(u8::try_from(size).unwrap_or(0)) else {
        return stop_with_bus_fault(uc, address, size, BusFault::InvalidSize);
    };
    let outcome = uc
        .get_data_mut()
        .bus
        .as_mut()
        .ok_or(BusFault::Unmapped)
        .and_then(|bus| bus.read(Addr(address), width));
    match outcome {
        Ok(value) => value,
        Err(fault) => stop_with_bus_fault(uc, address, size, fault),
    }
}

/// MMIO write at an absolute guest address.
fn mmio_write_at(uc: &mut Unicorn<'_, EngineData>, address: u64, size: usize, value: u64) {
    let Ok(width) = AccessSize::try_from(u8::try_from(size).unwrap_or(0)) else {
        stop_with_bus_fault(uc, address, size, BusFault::InvalidSize);
        return;
    };
    let outcome = uc
        .get_data_mut()
        .bus
        .as_mut()
        .ok_or(BusFault::Unmapped)
        .and_then(|bus| bus.write(Addr(address), width, value));
    if let Err(fault) = outcome {
        stop_with_bus_fault(uc, address, size, fault);
    }
}

/// Record the fault for [`Common::take_bus_fault`] and ask the backend to
/// stop at the current instruction. The dummy read value is never consumed
/// because emulation halts before the guest uses it.
fn stop_with_bus_fault(
    uc: &mut Unicorn<'_, EngineData>,
    address: u64,
    size: usize,
    fault: BusFault,
) -> u64 {
    let width = u8::try_from(size).unwrap_or(0);
    uc.get_data_mut().fault = Some(EngineBusFault::new(Addr(address), width, fault));
    let _ = uc.emu_stop();
    0
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use mosaic_isa_arm::Register;

    /// IA-32 fixture: 64 KiB of RAM at physical 0 plus a port recorder.
    fn x86_engine(ports: Option<Box<dyn PortBus>>) -> UnicornX86 {
        let mut engine = UnicornX86::new().unwrap();
        engine
            .map_memory(Addr(0), 0x1_0000, Permissions::RWX)
            .unwrap();
        if let Some(ports) = ports {
            engine.attach_ports(ports).unwrap();
        }
        engine
    }

    #[test]
    fn x86_unclaimed_exception_stops_the_quantum_and_names_the_vector() {
        // Unicorn never delivers an exception through the guest's IDT: it calls
        // UC_HOOK_INTR and moves on. With no service attached the engine must
        // stop instead of silently skipping the fault, and the report has to
        // say which vector it was.
        let mut engine = UnicornX86Boot::new().unwrap();
        engine
            .map_memory(Addr(0), 0x10_0000, Permissions::RWX)
            .unwrap();
        // xor ecx,ecx ; div ecx  -> #DE (vector 0)
        engine
            .write_memory(Addr(0x500), &[0x31, 0xC9, 0xF7, 0xF1])
            .unwrap();
        engine
            .reg_write(mosaic_isa_x86::Register::Eip.into(), 0x500)
            .unwrap();
        let outcome = engine.run(NonZeroU64::new(4).unwrap());
        assert_eq!(outcome.reason, StopReason::CapabilityMissing);
        // No gate table exists yet, so there is nowhere to deliver `#DE`: the
        // quantum stops and names the vector instead of skipping the fault.
        assert_eq!(engine.take_unclaimed_interrupt(), Some(0));
        assert_eq!(engine.take_exception(), Some((0, 0x502, 0)));
    }

    #[test]
    fn x86_boot_core_resumes_across_a_real_mode_far_jump() {
        use mosaic_isa_x86::Register;
        // `MODE_16`'s `uc_emu_start` rewrites the start address as a 16-bit IP
        // (`ip = begin - cs * 16`), so a context parked anywhere but the
        // segment base cannot be resumed. The boot core runs on the 32-bit mode
        // word instead and keeps real-mode semantics, so the same far jump
        // resumes on the next instruction (ADR 0020).
        let mut engine = UnicornX86Boot::new().unwrap();
        engine
            .map_memory(Addr(0), 0x10_0000, Permissions::RWX)
            .unwrap();
        // At 0x500: jmp far 9020:0000.
        engine
            .write_memory(Addr(0x500), &[0xEA, 0x00, 0x00, 0x20, 0x90])
            .unwrap();
        // Setup entry: jmp +0x6a, then `mov ax,0x1234 ; mov bx,0x5678`.
        engine.write_memory(Addr(0x90_200), &[0xEB, 0x6A]).unwrap();
        engine
            .write_memory(Addr(0x90_26C), &[0xB8, 0x34, 0x12, 0xBB, 0x78, 0x56])
            .unwrap();
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x500).unwrap();
        let mut eips = Vec::new();
        for _ in 0..4 {
            engine.run(NonZeroU64::new(1).unwrap());
            eips.push((
                engine.reg_read(Register::Cs.into()).unwrap(),
                engine.reg_read(Register::Eip.into()).unwrap(),
            ));
        }
        assert_eq!(
            eips,
            vec![(0x9020, 0), (0x9020, 0x6C), (0x9020, 0x6F), (0x9020, 0x72)],
            "each quantum must retire exactly one instruction and resume in the segment"
        );
        assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x1234);
        assert_eq!(engine.reg_read(Register::Ebx.into()).unwrap(), 0x5678);
    }

    #[derive(Default)]
    struct RecordingPorts {
        writes: Vec<(u16, usize, u32)>,
        reads: Vec<u16>,
        input: u32,
    }

    impl PortBus for RecordingPorts {
        fn read(&mut self, port: u16, _size: usize) -> Result<u32, BusFault> {
            self.reads.push(port);
            Ok(self.input)
        }

        fn write(&mut self, port: u16, size: usize, value: u32) -> Result<(), BusFault> {
            self.writes.push((port, size, value));
            Ok(())
        }
    }

    struct RecordingInterrupts {
        seen: Vec<u8>,
    }

    impl InterruptService for RecordingInterrupts {
        fn service(&mut self, vector: u8, cpu: &mut dyn SoftwareInterruptCpu) -> bool {
            self.seen.push(vector);
            // Report a value through AX, as a BIOS service would.
            let _ = cpu.reg_write(RegId(0), 0xB10C);
            true
        }
    }

    #[test]
    fn x86_software_interrupt_service_answers_and_resumes_after_the_int() {
        use mosaic_isa_x86::Register;
        let mut engine = x86_engine(None);
        engine
            .attach_interrupt_service(Box::new(RecordingInterrupts { seen: Vec::new() }))
            .unwrap();
        // int 0x10 ; mov ah, 0x4C ; hlt
        let program = [0xCD, 0x10, 0xB4, 0x4C, 0xF4];
        engine.write_memory(Addr(0x100), &program).unwrap();
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x100).unwrap();
        engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
        engine.run(NonZeroU64::new(2).unwrap());
        // The service wrote AX = 0xB10C and the guest then executed
        // `mov ah, 0x4C`, so AX ends as 0x4C0C: the service ran, and the CPU
        // resumed after the two-byte INT encoding instead of re-executing it.
        assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x4C0C);
        assert!(
            engine.reg_read(Register::Eip.into()).unwrap() >= 0x103,
            "EIP must be past the INT instruction"
        );
    }

    #[test]
    fn x86_real_mode_interrupt_pushes_a_16_bit_frame_from_the_ivt() {
        use mosaic_isa_x86::{eflags, Register};
        let mut engine = x86_engine(None);
        // Handler at 0000:0100 spins in place.
        engine.write_memory(Addr(0x100), &[0xEB, 0xFE]).unwrap();
        // IVT entry for vector 0x20 -> 0000:0100.
        let mut entry = Vec::new();
        entry.extend_from_slice(&0x0100u16.to_le_bytes());
        entry.extend_from_slice(&0x0000u16.to_le_bytes());
        engine.write_memory(Addr(0x20 * 4), &entry).unwrap();
        // Interrupted context at 0000:0200 with IF set.
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x200).unwrap();
        engine.reg_write(Register::Ss.into(), 0).unwrap();
        engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
        engine
            .reg_write(Register::Eflags.into(), u64::from(eflags::IF))
            .unwrap();

        engine.queue_interrupt(0x20);
        assert_eq!(engine.pending_interrupts(), 1);
        engine.run(NonZeroU64::new(1).unwrap());

        assert_eq!(engine.pending_interrupts(), 0);
        assert_eq!(engine.reg_read(Register::Eip.into()).unwrap(), 0x100);
        // Frame: IP, CS, FLAGS at growing addresses from the new SP.
        let sp = engine.reg_read(Register::Esp.into()).unwrap();
        assert_eq!(sp, 0x8000 - 6);
        let mut frame = [0u8; 6];
        engine.read_memory(Addr(sp), &mut frame).unwrap();
        assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x0200);
        assert_eq!(u16::from_le_bytes([frame[2], frame[3]]), 0x0000);
        // The saved copy keeps the interrupted IF; the live EFLAGS loses it.
        assert_ne!(
            u16::from_le_bytes([frame[4], frame[5]]) & eflags::IF as u16,
            0,
            "the saved frame keeps the interrupted IF for IRET"
        );
        assert_eq!(
            engine.reg_read(Register::Eflags.into()).unwrap() & u64::from(eflags::IF),
            0,
            "real-mode entry clears IF"
        );
    }

    #[test]
    fn x86_exception_enters_the_real_mode_ivt_handler() {
        use mosaic_isa_x86::Register;
        // Unicorn reports `#DE` to `UC_HOOK_INTR` and then continues without
        // entering any handler, so a real-mode guest's own IVT vector is the
        // adapter's job. The frame must be the one `iret` expects.
        let mut engine = x86_engine(None);
        // 0000:1000 -> `mov bx,0xDEAD ; hlt`.
        engine
            .write_memory(Addr(0x1000), &[0xBB, 0xAD, 0xDE, 0xF4])
            .unwrap();
        let mut entry = Vec::new();
        entry.extend_from_slice(&0x1000u16.to_le_bytes());
        entry.extend_from_slice(&0x0000u16.to_le_bytes());
        engine.write_memory(Addr(0), &entry).unwrap();
        // xor ecx,ecx ; div ecx -> #DE at 0000:0502.
        engine
            .write_memory(Addr(0x500), &[0x31, 0xC9, 0xF7, 0xF1])
            .unwrap();
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x500).unwrap();
        engine.reg_write(Register::Ss.into(), 0).unwrap();
        engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
        engine.run(NonZeroU64::new(4).unwrap());

        assert_eq!(
            engine.take_exception(),
            Some((0, 0x502, 0)),
            "the fault is reported where it happened"
        );
        assert_eq!(engine.take_unclaimed_interrupt(), None);
        let handler_eip = engine.reg_read(Register::Eip.into()).unwrap();
        assert!(
            (0x1000..0x1010).contains(&handler_eip),
            "the IVT handler runs, EIP {handler_eip:#x}"
        );
        assert_eq!(engine.reg_read(Register::Cs.into()).unwrap(), 0);
        assert_eq!(
            engine.reg_read(Register::Esp.into()).unwrap(),
            0x8000 - 6,
            "a real-mode frame is three 16-bit words"
        );
        let mut frame = [0u8; 6];
        engine.read_memory(Addr(0x8000 - 6), &mut frame).unwrap();
        assert_eq!(
            u16::from_le_bytes([frame[0], frame[1]]),
            0x502,
            "IP points at the faulting division"
        );
        assert_eq!(u16::from_le_bytes([frame[2], frame[3]]), 0);
        // The handler runs inside the same quantum: `mov bx,0xDEAD`.
        assert_eq!(engine.reg_read(Register::Ebx.into()).unwrap(), 0xDEAD);
    }

    #[test]
    fn x86_page_fault_enters_the_idt_gate_with_an_error_code() {
        use mosaic_isa_x86::Register;
        // The WP-bit probe every Linux x86 kernel runs is a deliberately
        // read-only page: it proves the CPU honours `CR0.WP` by faulting on a
        // supervisor write. The kernel expects the hardware to push
        // `error code / EIP / CS / EFLAGS` and vector through `IDT[14]`, and
        // the error code has to say "present, write" or the handler takes the
        // wrong branch.
        let mut engine = UnicornX86Boot::new().unwrap();
        engine
            .map_memory(Addr(0), 0x10_0000, Permissions::RWX)
            .unwrap();

        // GDT: null, 0x08 flat 32-bit code, 0x10 flat 32-bit data.
        let mut gdt = Vec::new();
        gdt.extend_from_slice(&[0u8; 8]);
        gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x9A, 0xCF, 0x00]);
        gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x92, 0xCF, 0x00]);
        engine.write_memory(Addr(0x2000), &gdt).unwrap();
        let mut gdtr = Vec::new();
        gdtr.extend_from_slice(&((gdt.len() - 1) as u16).to_le_bytes());
        gdtr.extend_from_slice(&0x2000u32.to_le_bytes());
        engine.write_memory(Addr(0x2100), &gdtr).unwrap();

        // IDT with one gate: vector 14 -> 0x08:0x6000, 32-bit interrupt gate.
        let mut idt = vec![0u8; 14 * 8];
        idt.extend_from_slice(&[0x00, 0x60, 0x08, 0x00, 0x00, 0x8E, 0x00, 0x00]);
        engine.write_memory(Addr(0x2200), &idt).unwrap();
        let mut idtr = Vec::new();
        idtr.extend_from_slice(&((idt.len() - 1) as u16).to_le_bytes());
        idtr.extend_from_slice(&0x2200u32.to_le_bytes());
        engine.write_memory(Addr(0x2110), &idtr).unwrap();

        // Identity map the first 4 MiB through 4 KiB pages; page 0x90 is
        // present but read-only, which is the page the guest will write.
        let mut page_table = Vec::with_capacity(4096);
        for index in 0..1024u32 {
            let entry = if index == 0x90 {
                0x90_0001u32
            } else {
                (index << 12) | 3
            };
            page_table.extend_from_slice(&entry.to_le_bytes());
        }
        engine.write_memory(Addr(0x4000), &page_table).unwrap();
        engine
            .write_memory(Addr(0x3000), &(0x4000u32 | 3).to_le_bytes())
            .unwrap();

        // Handler at 0x6000: `mov eax,0x0BAD ; hlt`. It must not share a page
        // with anything else the test patches: a handler written over the page
        // tables silently rewrites the identity map.
        engine
            .write_memory(Addr(0x6000), &[0xB8, 0xAD, 0x0B, 0x00, 0x00, 0xF4])
            .unwrap();

        // Real-mode prologue: the default operand size is 16 bits until a
        // 32-bit code segment is loaded, so the `CR0` write is spelled with an
        // operand-size prefix and the far jump with a 32-bit offset.
        let mut code = Vec::new();
        // lgdt [0x2100]
        code.extend_from_slice(&[0x0F, 0x01, 0x16, 0x00, 0x21]);
        // or eax, CR0.PG | CR0.WP | CR0.PE
        code.extend_from_slice(&[0x66, 0x0D, 0x01, 0x00, 0x01, 0x80]);
        // mov cr0, eax
        code.extend_from_slice(&[0x0F, 0x22, 0xC0]);
        let far_jump_at = code.len();
        code.extend_from_slice(&[0x66, 0xEA, 0, 0, 0, 0, 0x08, 0x00]);
        let entry_at = code.len();
        let entry = 0x1000u32 + entry_at as u32;
        code[far_jump_at + 2..far_jump_at + 6].copy_from_slice(&entry.to_le_bytes());
        // mov ax,0x10 ; mov ds,ax ; mov ss,ax ; mov esp,0x80000 ;
        // lidt [0x2110] ; mov byte [0x90000],1 ; hlt
        code.extend_from_slice(&[0x66, 0xB8, 0x10, 0x00]);
        code.extend_from_slice(&[0x8E, 0xD8]);
        code.extend_from_slice(&[0x8E, 0xD0]);
        code.extend_from_slice(&[0xBC, 0x00, 0x00, 0x08, 0x00]);
        code.extend_from_slice(&[0x0F, 0x01, 0x1D, 0x10, 0x21, 0x00, 0x00]);
        let store_at = 0x1000u32 + code.len() as u32;
        code.extend_from_slice(&[0xC6, 0x05, 0x00, 0x00, 0x09, 0x00, 0x01]);
        code.extend_from_slice(&[0xF4]);
        engine.write_memory(Addr(0x1000), &code).unwrap();

        // CR3 lives at the ISA's register 18; the page directory is above the
        // low page tables so a mistake shows up as a fault, not a silent pass.
        // Unicorn leaves `CR4.PAE` set on this backend; a 32-bit guest that
        // installs two-level tables has to clear it, exactly as a non-PAE
        // kernel does in its own `CR4` write.
        engine.reg_write(RegId(19), 0).unwrap();
        engine.reg_write(RegId(18), 0x3000).unwrap();
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x1000).unwrap();
        engine.run(NonZeroU64::new(64).unwrap());

        assert_eq!(
            engine.take_exception(),
            Some((14, u64::from(store_at), 0x9_0000)),
            "#PF is reported with the faulting instruction and CR2"
        );
        let handler_eip = engine.reg_read(Register::Eip.into()).unwrap();
        assert!(
            (0x6000..0x6010).contains(&handler_eip),
            "the guest's own #PF handler runs, EIP {handler_eip:#x}"
        );
        assert_eq!(engine.reg_read(Register::Cs.into()).unwrap(), 0x08);
        assert_eq!(
            engine.reg_read(Register::Esp.into()).unwrap(),
            0x80000 - 16,
            "error code + three word frame"
        );
        let mut frame = [0u8; 16];
        engine.read_memory(Addr(0x80000 - 16), &mut frame).unwrap();
        let word = |at: usize| {
            u32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]])
        };
        assert_eq!(word(0), 3, "error code: present | write");
        assert_eq!(word(4), store_at, "EIP points at the faulting store");
        assert_eq!(word(8), 0x08, "CS of the faulting context");
        assert_ne!(word(12) & 0x2, 0, "the frame carries a real EFLAGS image");
        assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x0BAD);
    }

    #[test]
    fn x86_masked_interrupt_stays_queued_until_if_is_set() {
        use mosaic_isa_x86::Register;
        let mut engine = x86_engine(None);
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x200).unwrap();
        engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
        engine.reg_write(Register::Eflags.into(), 0).unwrap();
        engine.queue_interrupt(0x20);
        engine.run(NonZeroU64::new(4).unwrap());
        assert_eq!(engine.pending_interrupts(), 1);
        assert_eq!(
            engine.reg_read(Register::Esp.into()).unwrap(),
            0x8000,
            "a masked request must not touch the stack"
        );
    }

    #[test]
    fn x86_port_io_reaches_the_port_bus_in_both_directions() {
        use mosaic_isa_x86::Register;
        let mut engine = x86_engine(Some(Box::new(RecordingPorts {
            input: 0x5A,
            ..RecordingPorts::default()
        })));
        // mov dx, 0x3F8 ; mov al, 0x41 ; out dx, al ; in al, dx ; hlt
        let program = [0xBA, 0xF8, 0x03, 0xB0, 0x41, 0xEE, 0xEC, 0xF4];
        engine.write_memory(Addr(0x100), &program).unwrap();
        engine.reg_write(Register::Cs.into(), 0).unwrap();
        engine.reg_write(Register::Eip.into(), 0x100).unwrap();
        let outcome = engine.run(NonZeroU64::new(5).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted, "{outcome:?}");
        // The recorder is owned by the engine now; assert through the guest AL.
        assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x5A);
    }

    #[test]
    fn x86_protected_mode_interrupt_uses_the_gate_and_masks_if() {
        use mosaic_isa_x86::{eflags, Register};
        let mut engine = UnicornX86::protected_mode().unwrap();
        engine
            .map_memory(Addr(0), 0x1_0000, Permissions::RWX)
            .unwrap();
        // GDT at 0x1000: null, flat code (0x08), flat data (0x10).
        let mut gdt = Vec::new();
        gdt.extend_from_slice(&[0u8; 8]);
        gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x9A, 0xCF, 0x00]);
        gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x92, 0xCF, 0x00]);
        engine.write_memory(Addr(0x1000), &gdt).unwrap();
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&((gdt.len() as u16) - 1).to_le_bytes());
        pseudo.extend_from_slice(&0x1000u32.to_le_bytes());
        engine.write_memory(Addr(0x400), &pseudo).unwrap();
        // IDT at 0x2000, gate 0x20 -> 0x0008:0x0300, type 0x8E (interrupt gate).
        let mut idt = vec![0u8; 8 * 0x21];
        idt[0x20 * 8..0x20 * 8 + 8]
            .copy_from_slice(&[0x00, 0x03, 0x08, 0x00, 0x00, 0x8E, 0x00, 0x00]);
        engine.write_memory(Addr(0x2000), &idt).unwrap();
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&((idt.len() as u16) - 1).to_le_bytes());
        pseudo.extend_from_slice(&0x2000u32.to_le_bytes());
        engine.write_memory(Addr(0x410), &pseudo).unwrap();
        engine.write_memory(Addr(0x300), &[0xEB, 0xFE]).unwrap();
        // lgdt [0x400] ; lidt [0x410] ; jmp $
        let program = [
            0x0F, 0x01, 0x15, 0x00, 0x04, 0x00, 0x00, 0x0F, 0x01, 0x1D, 0x10, 0x04, 0x00, 0x00,
            0xEB, 0xFE,
        ];
        engine.write_memory(Addr(0x200), &program).unwrap();
        engine.reg_write(Register::Eip.into(), 0x200).unwrap();
        engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
        engine
            .reg_write(Register::Eflags.into(), u64::from(eflags::IF))
            .unwrap();
        // Load the descriptor tables while the core is still un-protected, then
        // enter protected mode and reload the flat selectors from the GDT.
        engine.run(NonZeroU64::new(2).unwrap());
        engine.reg_write(Register::Cr0.into(), 1).unwrap();
        engine.reg_write(Register::Cs.into(), 0x08).unwrap();
        engine.reg_write(Register::Ds.into(), 0x10).unwrap();
        engine.reg_write(Register::Ss.into(), 0x10).unwrap();
        // Park the interrupted context on a two-byte spin instruction.
        engine.write_memory(Addr(0x220), &[0xEB, 0xFE]).unwrap();
        engine.reg_write(Register::Eip.into(), 0x220).unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.reg_read(Register::Eip.into()).unwrap(), 0x220);

        engine.queue_interrupt(0x20);
        engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.pending_interrupts(), 0);
        assert_eq!(engine.reg_read(Register::Eip.into()).unwrap(), 0x300);
        assert_eq!(engine.reg_read(Register::Cs.into()).unwrap(), 0x08);
        let esp = engine.reg_read(Register::Esp.into()).unwrap();
        assert_eq!(esp, 0x8000 - 12, "32-bit gate pushes a 12-byte frame");
        let mut frame = [0u8; 12];
        engine.read_memory(Addr(esp), &mut frame).unwrap();
        assert_eq!(
            u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]),
            0x220,
            "return EIP is the interrupted instruction"
        );
        assert_eq!(
            u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]),
            0x08
        );
        assert_ne!(
            u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) & eflags::IF,
            0,
            "the saved frame keeps the interrupted IF so IRET restores it"
        );
        assert_eq!(
            engine.reg_read(Register::Eflags.into()).unwrap() & u64::from(eflags::IF),
            0,
            "the live EFLAGS loses IF"
        );
    }

    fn engine() -> UnicornArm {
        let mut engine = UnicornArm::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine
    }

    #[test]
    fn mclass_backend_executes_t32_from_reset() {
        // ADR 0012: the M-class construction (Cortex-M33 via UC_MODE_MCLASS,
        // T bit asserted through the R15 write path) runs T32 from reset,
        // which the A32 backend cannot offer. Listing: MOVS r0,#42 (0x202A) ;
        // MOVS r1,#1 (0x2101) ; ADDS r0,r0,r1 (0x1840) ;
        // STR r1,[r2,#0] (0x6011).
        let mut engine = UnicornCortexM3::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        engine
            .write_memory(
                Addr(0x1000),
                &[0x2a, 0x20, 0x01, 0x21, 0x40, 0x18, 0x11, 0x60],
            )
            .unwrap();
        engine.reg_write(Register::R2.into(), 0x1100).unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        let outcome = engine.run(NonZeroU64::new(4).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(outcome.instruction_hooks, 4);
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(43));
        assert_eq!(engine.reg_read(Register::Pc.into()), Ok(0x1008));
        let mut bytes = [0; 4];
        engine.read_memory(Addr(0x1100), &mut bytes).unwrap();
        assert_eq!(bytes[0], 1);
        // M-profile PC writes are halfword aligned; odd values stay invalid.
        engine.reg_write(Register::Pc.into(), 0x1002).unwrap();
        assert_eq!(
            engine.reg_write(Register::Pc.into(), 0x1003),
            Err(EngineError::InvalidValue)
        );
        // The A32 backend still rejects halfword targets outright.
        let mut a32 = UnicornArm::new().unwrap();
        a32.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        assert_eq!(
            a32.reg_write(Register::Pc.into(), 0x1002),
            Err(EngineError::InvalidValue)
        );
    }

    #[test]
    fn context_save_and_restore_preserves_registers() {
        let mut engine = engine();
        engine.reg_write(Register::R0.into(), 123).unwrap();
        engine.reg_write(Register::R1.into(), 456).unwrap();
        let ctx = engine.save_context().unwrap();
        engine.reg_write(Register::R0.into(), 999).unwrap();
        engine.reg_write(Register::R1.into(), 888).unwrap();
        engine.restore_context(&ctx).unwrap();
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(123));
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(456));
    }

    #[test]
    fn x86_64_backend_executes_instructions() {
        use mosaic_isa_x86_64::Register as X86Reg;
        let mut engine = UnicornX86_64::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        // nop (0x90), nop (0x90), jmp $ (0xeb, 0xfe)
        engine
            .write_memory(Addr(0x1000), &[0x90, 0x90, 0xeb, 0xfe])
            .unwrap();
        engine.reg_write(X86Reg::Rip.into(), 0x1000).unwrap();
        let outcome = engine.run(NonZeroU64::new(2).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(engine.reg_read(X86Reg::Rip.into()), Ok(0x1002));
    }

    #[test]
    fn arm64_backend_executes_instructions() {
        use mosaic_isa_aarch64::Register as Arm64Reg;
        let mut engine = UnicornArm64::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        // nop (0xd503201f le), b . (0x14000000 le)
        engine
            .write_memory(
                Addr(0x1000),
                &[0x1f, 0x20, 0x03, 0xd5, 0x00, 0x00, 0x00, 0x14],
            )
            .unwrap();
        engine.reg_write(Arm64Reg::Pc.into(), 0x1000).unwrap();
        let outcome = engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(engine.reg_read(Arm64Reg::Pc.into()), Ok(0x1004));
    }

    #[test]
    fn instruction_budget_and_resume_are_exact_for_a32() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2])
            .unwrap();
        for expected_pc in [0x1004, 0x1008] {
            let result = engine.run(NonZeroU64::new(1).unwrap());
            assert_eq!(result.reason, StopReason::BudgetExhausted);
            assert_eq!(result.instruction_hooks, 1);
            assert_eq!(engine.reg_read(Register::Pc.into()), Ok(expected_pc));
        }
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(42));
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(43));
    }

    #[test]
    fn patching_code_invalidates_translated_blocks() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[1, 0, 0xa0, 0xe3])
            .unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(1));
        engine
            .write_memory(Addr(0x1000), &[2, 0, 0xa0, 0xe3])
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(2));
    }

    #[test]
    fn guest_permissions_are_not_bypassed_by_loader_access() {
        let mut engine = UnicornArm::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RX)
            .unwrap();
        engine
            .map_memory(Addr(0x2000), PAGE_SIZE, Permissions::R)
            .unwrap();
        // MOV r0,#42; STR r0,[r2]. Loader may initialize RX; guest may not write R.
        engine
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5])
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine.reg_write(Register::R2.into(), 0x2000).unwrap();
        assert_eq!(
            engine.run(NonZeroU64::new(2).unwrap()).reason,
            StopReason::Fault
        );
        let mut data = [0; 4];
        engine.read_memory(Addr(0x2000), &mut data).unwrap();
        assert_eq!(data, [0; 4]);

        let mut no_execute = UnicornArm::new().unwrap();
        no_execute
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW)
            .unwrap();
        no_execute
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3])
            .unwrap();
        no_execute.reg_write(Register::Pc.into(), 0x1000).unwrap();
        assert_eq!(
            no_execute.run(NonZeroU64::new(1).unwrap()).reason,
            StopReason::Fault
        );
    }

    #[test]
    fn an_infinite_guest_loop_stops_at_the_instruction_budget() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[0xfe, 0xff, 0xff, 0xea])
            .unwrap(); // B .
        let outcome = engine.run(NonZeroU64::new(20).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(outcome.instruction_hooks, 20);
        assert_eq!(engine.reg_read(Register::Pc.into()), Ok(0x1000));
    }

    #[test]
    fn mapped_pages_are_private_to_each_instance() {
        let mut first = engine();
        let mut second = engine();
        first.write_memory(Addr(0x1100), &[1, 2, 3, 4]).unwrap();
        second.write_memory(Addr(0x1100), &[5, 6, 7, 8]).unwrap();
        let mut bytes = [0; 4];
        first.read_memory(Addr(0x1100), &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
        second.read_memory(Addr(0x1100), &mut bytes).unwrap();
        assert_eq!(bytes, [5, 6, 7, 8]);
    }

    #[test]
    fn shared_ram_is_coherent_across_engine_instances() {
        // Design §12.3 rule 1 verification. Unicorn 2.1.x maps host pointers
        // read-only towards the guest, so the shared page is mirrored per
        // engine and synchronised at deterministic boundaries (ADR 0010).
        let ram = SharedRam::new(PAGE_SIZE).unwrap();
        let mut first = UnicornArm::new().unwrap();
        let mut second = UnicornArm::new().unwrap();
        for core in [&mut first, &mut second] {
            core.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
                .unwrap();
            core.map_shared_memory(Addr(0x10000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram))
                .unwrap();
        }
        // MOV r0,#0xAB; LDR r1,[pc,#4]; STRB r0,[r1]; B .; .word 0x00010000
        // The literal sits at +0x10 from the load, so the load uses +4.
        first
            .write_memory(
                Addr(0x1000),
                &[
                    0xab, 0, 0xa0, 0xe3, 4, 0x10, 0x9f, 0xe5, 0, 0, 0xc1, 0xe5, 0xfe, 0xff, 0xff,
                    0xea, 0, 0, 1, 0,
                ],
            )
            .unwrap();
        first.reg_write(Register::Pc.into(), 0x1000).unwrap();
        let outcome = first.run(NonZeroU64::new(3).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        // The guest store lands in the first engine's mirror.
        let mut bytes = [0; 4];
        first.read_memory(Addr(0x10000), &mut bytes).unwrap();
        assert_eq!(bytes[0], 0xab);
        // Copying the mirror back publishes it to the authoritative buffer.
        first.sync_shared_out().unwrap();
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes[0], 0xab);
        // The next core sees it after the copy in.
        second.sync_shared_in().unwrap();
        second.read_memory(Addr(0x10000), &mut bytes).unwrap();
        assert_eq!(bytes[0], 0xab);
        // Host-side writes reach the guests at the next copy in.
        ram.write(8, &[9]).unwrap();
        second.sync_shared_in().unwrap();
        second.read_memory(Addr(0x10008), &mut bytes[..1]).unwrap();
        assert_eq!(bytes[0], 9);
        // A guest write in the second core is published on copy out.
        second.write_memory(Addr(0x10004), &[7]).unwrap();
        second.sync_shared_out().unwrap();
        ram.read(4, &mut bytes[..1]).unwrap();
        assert_eq!(bytes[0], 7);
    }

    #[test]
    fn shared_mappings_validate_like_native_regions() {
        let ram = SharedRam::new(PAGE_SIZE).unwrap();
        let mut engine = UnicornArm::new().unwrap();
        assert!(matches!(
            SharedRam::new(0),
            Err(EngineError::InvalidMapping)
        ));
        // The buffer bounds the mapping size.
        assert_eq!(
            engine.map_shared_memory(
                Addr(0x1000),
                2 * PAGE_SIZE,
                Permissions::RW,
                Arc::clone(&ram)
            ),
            Err(EngineError::InvalidMapping)
        );
        engine
            .map_shared_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram))
            .unwrap();
        assert_eq!(
            engine.map_shared_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram)),
            Err(EngineError::Overlap)
        );
        assert_eq!(
            engine.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW),
            Err(EngineError::Overlap)
        );
        // Loader spans resolve against the shared mapping.
        assert_eq!(
            engine.write_memory(Addr(0x2000), &[1]),
            Err(EngineError::Unmapped)
        );
        assert_eq!(
            ram.write(PAGE_SIZE as usize, &[1]),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            ram.read(1, &mut [0u8; PAGE_SIZE as usize]),
            Err(EngineError::InvalidMapping)
        );
    }

    #[test]
    fn mapping_and_register_errors_do_not_panic() {
        let mut engine = engine();
        assert_eq!(
            engine.map_memory(Addr(1), PAGE_SIZE, Permissions::RW),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.map_memory(Addr(0x2000), 0, Permissions::RW),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW),
            Err(EngineError::Overlap)
        );
        assert_eq!(
            engine.map_memory(Addr(0xffff_f000), 2 * PAGE_SIZE, Permissions::RW),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.reg_read(RegId(99)),
            Err(EngineError::InvalidRegister)
        );
        assert_eq!(
            engine.reg_write(Register::R0.into(), 1 << 32),
            Err(EngineError::InvalidValue)
        );
        // A32 state requires 4-byte PC alignment; 2 mod 4 stays invalid.
        assert_eq!(
            engine.reg_write(Register::Pc.into(), 0x1002),
            Err(EngineError::InvalidValue)
        );
        assert_eq!(
            engine.write_memory(Addr(0x2000), &[0]),
            Err(EngineError::Unmapped)
        );
        assert_eq!(
            engine.read_memory(Addr(0x1fff), &mut [0; 2]),
            Err(EngineError::Unmapped)
        );
    }

    #[test]
    fn faults_and_unsupported_budgets_do_not_report_success() {
        let mut engine = UnicornArm::new().unwrap();
        assert_eq!(
            engine.run(NonZeroU64::new(1).unwrap()).reason,
            StopReason::Fault
        );
        assert_eq!(
            engine.run(NonZeroU64::new(MAX_QUANTUM + 1).unwrap()),
            RunOutcome {
                reason: StopReason::CapabilityMissing,
                instruction_hooks: 0,
            }
        );
        assert!(!engine.caps().exclusive_monitor);
        assert!(!engine.caps().precise_exceptions);
    }

    #[test]
    fn a_created_and_used_engine_can_move_between_threads_sequentially() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2])
            .unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        let mut engine = std::thread::spawn(move || {
            assert_eq!(
                engine.run(NonZeroU64::new(1).unwrap()).reason,
                StopReason::BudgetExhausted
            );
            engine
        })
        .join()
        .unwrap();
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(43));
        engine.write_memory(Addr(0x1100), &[7]).unwrap();
    }

    #[derive(Default)]
    struct BusRecord {
        writes: Vec<(u64, u8, u64)>,
        reads: Vec<(u64, u8)>,
    }

    struct FakeBus {
        record: Arc<Mutex<BusRecord>>,
        read_value: u64,
        fail_read: Option<BusFault>,
    }

    impl FakeBus {
        fn new(read_value: u64) -> (Self, Arc<Mutex<BusRecord>>) {
            let record = Arc::new(Mutex::new(BusRecord::default()));
            (
                Self {
                    record: Arc::clone(&record),
                    read_value,
                    fail_read: None,
                },
                record,
            )
        }

        fn failing(record: Arc<Mutex<BusRecord>>, fault: BusFault) -> Self {
            Self {
                record,
                read_value: 0,
                fail_read: Some(fault),
            }
        }
    }

    impl EngineBus for FakeBus {
        fn read(&mut self, address: Addr, width: AccessSize) -> Result<u64, BusFault> {
            self.record
                .lock()
                .unwrap()
                .reads
                .push((address.0, width.bytes()));
            match self.fail_read {
                Some(fault) => Err(fault),
                None => Ok(self.read_value),
            }
        }

        fn write(&mut self, address: Addr, width: AccessSize, value: u64) -> Result<(), BusFault> {
            self.record
                .lock()
                .unwrap()
                .writes
                .push((address.0, width.bytes(), value));
            Ok(())
        }
    }

    #[test]
    fn mmio_hooks_forward_guest_accesses_to_the_attached_bus() {
        let mut engine = engine();
        let (bus, record) = FakeBus::new(0x0bad_f00d);
        engine.attach_bus(Box::new(bus)).unwrap();
        engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
        // MOV r0,#0x55; STR r0,[r2]; LDR r1,[r2]; STRB r0,[r2,#4]; LDRB r3,[r2,#4]
        engine
            .write_memory(
                Addr(0x1000),
                &[
                    0x55, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5, 0, 0x10, 0x92, 0xe5, 4, 0, 0xc2, 0xe5,
                    4, 0x30, 0xd2, 0xe5,
                ],
            )
            .unwrap();
        engine.reg_write(Register::R2.into(), 0x2000).unwrap();
        let outcome = engine.run(NonZeroU64::new(5).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(outcome.instruction_hooks, 5);
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(0x55));
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(0x0bad_f00d));
        // LDRB keeps the low byte of the bus value.
        assert_eq!(engine.reg_read(Register::R3.into()), Ok(0x0d));
        let record = record.lock().unwrap();
        assert_eq!(record.writes, vec![(0x2000, 4, 0x55), (0x2004, 1, 0x55)]);
        assert_eq!(record.reads, vec![(0x2000, 4), (0x2004, 1)]);
        assert!(engine.take_bus_fault().is_none());
    }

    #[test]
    fn mmio_bus_fault_stops_emulation_and_is_reported_once() {
        let mut engine = engine();
        let record = Arc::new(Mutex::new(BusRecord::default()));
        let bus = FakeBus::failing(Arc::clone(&record), BusFault::Device);
        engine.attach_bus(Box::new(bus)).unwrap();
        engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
        // LDR r1,[r2] is the first instruction.
        engine
            .write_memory(Addr(0x1000), &[0, 0x10, 0x92, 0xe5])
            .unwrap();
        engine.reg_write(Register::R2.into(), 0x2000).unwrap();
        let outcome = engine.run(NonZeroU64::new(4).unwrap());
        assert_eq!(outcome.reason, StopReason::Fault);
        assert_eq!(
            engine.take_bus_fault(),
            Some(EngineBusFault::new(Addr(0x2000), 4, BusFault::Device))
        );
        assert!(engine.take_bus_fault().is_none());
        assert_eq!(record.lock().unwrap().reads, vec![(0x2000, 4)]);
    }

    #[test]
    fn mmio_registration_rejects_misuse_and_stays_out_of_loader_spans() {
        let mut engine = UnicornArm::new().unwrap();
        assert_eq!(
            engine.map_mmio(Addr(0x2000), PAGE_SIZE),
            Err(EngineError::BusNotAttached)
        );
        let (bus, _) = FakeBus::new(0);
        engine.attach_bus(Box::new(bus)).unwrap();
        assert_eq!(
            engine.attach_bus(Box::new(FakeBus::new(0).0)),
            Err(EngineError::BusAlreadyAttached)
        );
        assert_eq!(
            engine.map_mmio(Addr(1), PAGE_SIZE),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.map_mmio(Addr(0x2000), 0),
            Err(EngineError::InvalidMapping)
        );
        engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
        assert_eq!(
            engine.map_mmio(Addr(0x2000), PAGE_SIZE),
            Err(EngineError::Overlap)
        );
        assert_eq!(
            engine.map_memory(Addr(0x2000), PAGE_SIZE, Permissions::RW),
            Err(EngineError::Overlap)
        );
        // Callback regions are not reachable through the loader memory API.
        assert_eq!(
            engine.write_memory(Addr(0x2000), &[1]),
            Err(EngineError::Unmapped)
        );
    }

    #[test]
    fn set_line_records_edges_and_rejects_unknown_arm_lines() {
        let mut engine = UnicornArm::new().unwrap();
        assert!(!engine.caps().line_injection);
        engine.set_line(ARM_IRQ, true).unwrap();
        engine.set_line(ARM_FIQ, true).unwrap();
        engine.set_line(ARM_IRQ, false).unwrap();
        assert_eq!(
            engine.irq_edges(),
            &[(ARM_IRQ, true), (ARM_FIQ, true), (ARM_IRQ, false)]
        );
        // The contract view exposes the same log.
        assert_eq!(CpuEngine::line_edges(&engine).len(), 3);
        assert_eq!(engine.irq_level(ARM_IRQ), Some(false));
        assert_eq!(
            engine.set_line(IrqLine(9), true),
            Err(EngineError::LineNotSupported)
        );
    }

    #[cfg(feature = "arch-riscv")]
    mod riscv32 {
        use super::*;
        use mosaic_isa_riscv::Register as RvReg;

        fn engine() -> UnicornRiscV32 {
            let mut engine = UnicornRiscV32::new().unwrap();
            engine
                .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
                .unwrap();
            engine.reg_write(RvReg::Pc.into(), 0x1000).unwrap();
            engine
        }

        #[test]
        fn instruction_budget_and_resume_are_exact_for_rv32i() {
            let mut engine = engine();
            // addi x5,x0,42; addi x6,x5,1; addi x0,x0,5 (x0 stays hardwired)
            engine
                .write_memory(
                    Addr(0x1000),
                    &[
                        0x93, 0x02, 0xa0, 0x02, 0x13, 0x83, 0x12, 0x00, 0x13, 0x00, 0x50, 0x00,
                    ],
                )
                .unwrap();
            for expected_pc in [0x1004, 0x1008] {
                let result = engine.run(NonZeroU64::new(1).unwrap());
                assert_eq!(result.reason, StopReason::BudgetExhausted);
                assert_eq!(result.instruction_hooks, 1);
                assert_eq!(engine.reg_read(RvReg::Pc.into()), Ok(expected_pc));
            }
            assert_eq!(engine.reg_read(RvReg::X5.into()), Ok(42));
            assert_eq!(engine.reg_read(RvReg::X6.into()), Ok(43));
            let _ = engine.run(NonZeroU64::new(1).unwrap());
            assert_eq!(engine.reg_read(RvReg::X0.into()), Ok(0));
        }

        #[test]
        fn mmio_and_shared_paths_match_the_arm_adapter() {
            let mut engine = engine();
            let (bus, record) = FakeBus::new(0x0bad_f00d);
            engine.attach_bus(Box::new(bus)).unwrap();
            engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
            let ram = SharedRam::new(PAGE_SIZE).unwrap();
            engine
                .map_shared_memory(Addr(0x10000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram))
                .unwrap();
            // lw x6,0(x5); sw x6,0(x7): x5 = MMIO base, x7 = shared base.
            engine
                .write_memory(
                    Addr(0x1000),
                    &[0x03, 0xa3, 0x02, 0x00, 0x23, 0xa0, 0x63, 0x00],
                )
                .unwrap();
            engine.reg_write(RvReg::X5.into(), 0x2000).unwrap();
            engine.reg_write(RvReg::X7.into(), 0x10000).unwrap();
            let outcome = engine.run(NonZeroU64::new(2).unwrap());
            assert_eq!(outcome.reason, StopReason::BudgetExhausted);
            assert_eq!(engine.reg_read(RvReg::X6.into()), Ok(0x0bad_f00d));
            assert_eq!(record.lock().unwrap().reads, vec![(0x2000, 4)]);
            // The shared page is mirrored: the guest store lands in the
            // engine's copy and reaches the authoritative buffer on sync.
            let mut bytes = [0; 4];
            engine.read_memory(Addr(0x10000), &mut bytes).unwrap();
            assert_eq!(bytes, 0x0bad_f00du32.to_le_bytes());
            engine.sync_shared_out().unwrap();
            ram.read(0, &mut bytes).unwrap();
            assert_eq!(bytes, 0x0bad_f00du32.to_le_bytes());
        }

        #[test]
        fn lines_and_registers_reject_out_of_vocabulary_input() {
            let mut engine = UnicornRiscV32::new().unwrap();
            engine.set_line(RISCV_MEXT, true).unwrap();
            assert_eq!(CpuEngine::line_edges(&engine), &[(RISCV_MEXT, true)]);
            assert_eq!(
                engine.set_line(IrqLine(1), true),
                Err(EngineError::LineNotSupported)
            );
            assert_eq!(
                engine.reg_read(RegId(33)),
                Err(EngineError::InvalidRegister)
            );
            assert_eq!(
                engine.reg_write(RvReg::Pc.into(), 0x1002),
                Err(EngineError::InvalidValue)
            );
            assert_eq!(
                engine.reg_write(RvReg::X5.into(), 1u64 << 32),
                Err(EngineError::InvalidValue)
            );
        }

        #[test]
        fn the_register_table_matches_the_backend_enum() {
            for (index, register) in RISCV_GENERAL.iter().enumerate() {
                assert_eq!(
                    *register as i32,
                    RegisterRISCV::X0 as i32 + index as i32,
                    "X{index} must stay contiguous in uc_riscv_reg"
                );
            }
        }
    }
}
