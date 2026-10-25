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
    unicorn_const::{Arch, Mode, Prot},
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
    /// Bounded ring of recent instruction addresses, recorded only when a
    /// bring-up harness asks for it. A guest that faults leaves no other
    /// record of *where* it went, and a debugger is not always available.
    trace: Option<std::collections::VecDeque<u64>>,
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
            Backend::X86Protected => (Arch::X86, Mode::MODE_32 | Mode::LITTLE_ENDIAN),
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => (Arch::RISCV, Mode::MODE_32 | Mode::LITTLE_ENDIAN),
        };
        let mut uc = Unicorn::new_with_data(arch, mode, EngineData::default())
            .map_err(|_| EngineError::Backend)?;
        // Capture-free: no Rc/host reference can escape the engine through a hook.
        // Native count and MAX_QUANTUM bound the addition below to <= 1_000_000.
        uc.add_code_hook(1, 0, |uc, _, _| {
            uc.get_data_mut().counters.instructions += 1;
        })
        .map_err(|_| EngineError::Backend)?;
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
        if matches!(backend, Backend::X86Real | Backend::X86Protected) {
            // LGDT/LIDT are at least five bytes, so the 16-bit boot stub's
            // short instructions never reach the decoder, and the whole hook
            // switches off once both tables are known.
            uc.add_code_hook(1, 0, |uc, address, size| {
                if size < 5 {
                    return;
                }
                let tables = uc.get_data().tables;
                if tables.gdt.is_some() && tables.idt.is_some() {
                    return;
                }
                x86_observe_descriptor_load(uc, address);
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
                    // Nothing claimed the vector: stop the quantum so the
                    // caller can decide instead of silently skipping it.
                    let _ = uc.emu_stop();
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
            Backend::X86Real | Backend::X86Protected => match reg.0 {
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
            Backend::X86Real | Backend::X86Protected => {
                // External interrupts are taken at the quantum boundary: the
                // only point where a frame can be pushed without fighting an
                // in-flight instruction (ADR 0018).
                if self.x86_deliver_pending().is_err() {
                    return outcome(StopReason::Fault, 0);
                }
                let Ok(pc) = self.inner.0.reg_read(RegisterX86::EIP) else {
                    return outcome(StopReason::Fault, 0);
                };
                if matches!(self.backend, Backend::X86Real) {
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
        let result = self.inner.0.emu_start(begin, u64::MAX, 0, budget as usize);
        let observed = self.inner.0.get_data().counters.instructions;
        let reason = if result.is_err() || self.inner.0.get_data().fault.is_some() {
            StopReason::Fault
        } else if observed == budget {
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
            Backend::X86Real | Backend::X86Protected => 8,
        };
        if reg.0 == pc_id {
            let aligned = match self.backend {
                Backend::Arm | Backend::Arm64 => value % 4 == 0,
                Backend::ArmMClass => value % 2 == 0,
                Backend::X86_64 => true, // x86 instructions are byte aligned
                #[cfg(feature = "arch-riscv")]
                Backend::RiscV32 => value % 4 == 0,
                // x86 instructions are variable length; any EIP is legal.
                Backend::X86Real | Backend::X86Protected => true,
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
            Backend::X86Real | Backend::X86Protected => line.0 == 0,
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

/// IA-32 architectural interrupt delivery.
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
/// * a ring change through a conforming gate is not modelled yet, and
///   long-mode (64-bit) gates are rejected rather than half-implemented.
///
/// Instruction accounting is untouched: entry costs no guest instruction,
/// which matches the existing "budget = instructions executed" contract.
impl Common {
    fn x86_read_u16(&self, address: u64) -> Result<u16, EngineError> {
        let mut buf = [0u8; 2];
        self.inner
            .0
            .mem_read(address, &mut buf)
            .map_err(|_| EngineError::Backend)?;
        Ok(u16::from_le_bytes(buf))
    }

    fn x86_read_bytes(&self, address: u64, buf: &mut [u8]) -> Result<(), EngineError> {
        self.inner
            .0
            .mem_read(address, buf)
            .map_err(|_| EngineError::Backend)
    }

    fn x86_write_bytes(&mut self, address: u64, buf: &[u8]) -> Result<(), EngineError> {
        self.inner
            .0
            .mem_write(address, buf)
            .map_err(|_| EngineError::Backend)?;
        // The frame lives in ordinary RAM, but a stale translation block that
        // cached the old stack contents must not survive the write.
        let end = address + buf.len() as u64;
        self.inner
            .0
            .ctl_remove_cache(address, end)
            .map_err(|_| EngineError::Backend)
    }

    /// Linear base of a segment selector's descriptor in the current GDT.
    ///
    /// Flat-model guests (every catalog PC target) use base 0 descriptors, but
    /// the value is read from the guest table rather than assumed, so a
    /// small-model guest fails loudly instead of silently pushing its frame to
    /// the wrong address. An unobserved `GDTR` is a capability gap, not a guess.
    fn x86_descriptor_base(&self, selector: u16) -> Result<u64, EngineError> {
        if selector & 0x0004 != 0 {
            return Err(EngineError::CapabilityMissing);
        }
        let index = u64::from(selector >> 3);
        if index == 0 {
            return Ok(0);
        }
        let (base, limit) = self
            .inner
            .0
            .get_data()
            .tables
            .gdt
            .ok_or(EngineError::CapabilityMissing)?;
        let offset = index * 8;
        if offset + 7 > limit {
            return Err(EngineError::CapabilityMissing);
        }
        let mut raw = [0u8; 8];
        self.x86_read_bytes(base + offset, &mut raw)?;
        // Intel SDM Vol.3 Fig.3-8: base is split across three fields.
        let segment_base = u64::from(raw[2])
            | (u64::from(raw[3]) << 8)
            | (u64::from(raw[4]) << 16)
            | (u64::from(raw[7]) << 24);
        Ok(segment_base & 0xFFFF_FFFF)
    }

    /// `IDTR` as `(base, limit)`, as observed at the guest's `LIDT`.
    fn x86_idtr(&self) -> Result<(u64, u64), EngineError> {
        self.inner
            .0
            .get_data()
            .tables
            .idt
            .ok_or(EngineError::CapabilityMissing)
    }

    /// Deliver one pending vector if the guest can take it. `Ok(false)` means
    /// interrupts are disabled and the vector stays queued.
    fn x86_deliver_one(&mut self, vector: u8) -> Result<bool, EngineError> {
        let flags = self
            .inner
            .0
            .reg_read(RegisterX86::EFLAGS as i32)
            .map_err(|_| EngineError::Backend)? as u32;
        // IF gates INTR only; the queued request must survive until it is set.
        const IF: u32 = 1 << 9;
        const TF: u32 = 1 << 8;
        if flags & IF == 0 {
            return Ok(false);
        }
        let cr0 = self
            .inner
            .0
            .reg_read(RegisterX86::CR0 as i32)
            .map_err(|_| EngineError::Backend)? as u32;
        let cs = self
            .inner
            .0
            .reg_read(RegisterX86::CS as i32)
            .map_err(|_| EngineError::Backend)? as u16;
        let eip = self
            .inner
            .0
            .reg_read(RegisterX86::EIP as i32)
            .map_err(|_| EngineError::Backend)? as u32;
        let ss = self
            .inner
            .0
            .reg_read(RegisterX86::SS as i32)
            .map_err(|_| EngineError::Backend)? as u16;
        let esp = self
            .inner
            .0
            .reg_read(RegisterX86::ESP as i32)
            .map_err(|_| EngineError::Backend)? as u32;

        if cr0 & 1 == 0 {
            // Real mode: the interrupt table is the 256-entry IVT at 0x0 and
            // the pushed frame is always 16-bit wide.
            let entry = u64::from(vector) * 4;
            let handler_ip = self.x86_read_u16(entry)?;
            let handler_cs = self.x86_read_u16(entry + 2)?;
            let base = u64::from(ss) * 16;
            let mut sp = u32::from(esp as u16);
            for value in [flags as u16, cs, eip as u16] {
                sp = sp.wrapping_sub(2) & 0xFFFF;
                self.x86_write_bytes(base + u64::from(sp), &value.to_le_bytes())?;
            }
            self.inner
                .0
                .reg_write(RegisterX86::ESP as i32, u64::from(sp))
                .map_err(|_| EngineError::Backend)?;
            // Real-mode interrupt entry clears IF and TF unconditionally.
            self.inner
                .0
                .reg_write(RegisterX86::EFLAGS as i32, u64::from(flags & !(IF | TF)))
                .map_err(|_| EngineError::Backend)?;
            self.inner
                .0
                .reg_write(RegisterX86::CS as i32, u64::from(handler_cs))
                .map_err(|_| EngineError::Backend)?;
            self.inner
                .0
                .reg_write(RegisterX86::EIP as i32, u64::from(handler_ip))
                .map_err(|_| EngineError::Backend)?;
            return Ok(true);
        }

        let (idtr_base, idtr_limit) = self.x86_idtr()?;
        let gate_at = u64::from(vector) * 8;
        if gate_at + 7 > idtr_limit {
            return Err(EngineError::CapabilityMissing);
        }
        let mut raw = [0u8; 8];
        self.x86_read_bytes(idtr_base + gate_at, &mut raw)?;
        let low = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let _high = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        // Intel SDM Vol.3 Fig.6-2: byte 5 is P(1) DPL(2) S(1) Type(4).
        let present = raw[5] & 0x80 != 0;
        let gate_type = raw[5] & 0x0F;
        if !present {
            return Err(EngineError::CapabilityMissing);
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
            self.x86_descriptor_base(ss)?
        };
        let mut sp = esp;
        if wide {
            for value in [flags, u32::from(cs), eip] {
                sp = sp.wrapping_sub(4);
                self.x86_write_bytes(ss_base + u64::from(sp), &value.to_le_bytes())?;
            }
        } else {
            for value in [flags as u16, cs, eip as u16] {
                sp = sp.wrapping_sub(2);
                self.x86_write_bytes(ss_base + u64::from(sp), &value.to_le_bytes())?;
            }
        }
        self.inner
            .0
            .reg_write(RegisterX86::ESP as i32, u64::from(sp))
            .map_err(|_| EngineError::Backend)?;
        if crate::x86_gate_clears_if(gate_type) {
            self.inner
                .0
                .reg_write(RegisterX86::EFLAGS as i32, u64::from(flags & !(IF | TF)))
                .map_err(|_| EngineError::Backend)?;
        }
        self.inner
            .0
            .reg_write(RegisterX86::CS as i32, u64::from(selector))
            .map_err(|_| EngineError::Backend)?;
        self.inner
            .0
            .reg_write(RegisterX86::EIP as i32, handler)
            .map_err(|_| EngineError::Backend)?;
        Ok(true)
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
    if uc.mem_read(address, &mut opcode).is_err() || opcode[0] != 0x0F || opcode[1] != 0x01 {
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
    if uc.mem_read(operand, &mut descriptor).is_err() {
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
            uc.mem_read(at, &mut disp).ok()?;
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
                uc.mem_read(at, &mut disp).ok()?;
                base = base.wrapping_add(disp[0] as i8 as i64 as u64);
            }
            _ => {
                let mut disp = [0u8; 2];
                uc.mem_read(at, &mut disp).ok()?;
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
        uc.mem_read(at, &mut disp).ok()?;
        return Some(u64::from(u32::from_le_bytes(disp)));
    }
    if rm == 0b100 {
        let mut sib = [0u8; 1];
        uc.mem_read(at, &mut sib).ok()?;
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
                    uc.mem_read(at, &mut disp).ok()?;
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
            uc.mem_read(at, &mut disp).ok()?;
            offset = offset.wrapping_add(disp[0] as i8 as i64 as u64);
        }
        _ => {
            let mut disp = [0u8; 4];
            uc.mem_read(at, &mut disp).ok()?;
            offset = offset.wrapping_add(u64::from(u32::from_le_bytes(disp)));
        }
    }
    Some(offset & 0xFFFF_FFFF)
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

impl UnicornX86 {
    /// IA-32 with a 32-bit default operand size. `CR0.PE` is still guest
    /// owned: the guest's `CR0` write is what switches the decoder to
    /// protected-mode semantics, so only the initial mode word differs.
    pub fn protected_mode() -> Result<Self, EngineError> {
        Ok(Self(Common::new(Backend::X86Protected)?))
    }

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
        self.0.x86_idtr()
    }

    /// `GDTR` descriptor base for `selector`, resolved through the guest GDT.
    pub fn segment_base(&self, selector: u16) -> Result<u64, EngineError> {
        self.0.x86_descriptor_base(selector)
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
            .add_code_hook(1, 0, move |uc, address, _| {
                let data = uc.get_data_mut();
                if let Some(trace) = data.trace.as_mut() {
                    if trace.len() == capacity {
                        trace.pop_front();
                    }
                    trace.push_back(address);
                }
            })
            .map_err(|_| EngineError::Backend)?;
        Ok(())
    }

    /// The recorded instruction addresses, oldest first.
    pub fn instruction_trace(&self) -> Vec<u64> {
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
