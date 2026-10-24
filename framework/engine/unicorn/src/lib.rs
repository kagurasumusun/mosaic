#![deny(unsafe_code)]
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
    RegisterARM, Unicorn,
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
    fault: Option<EngineBusFault>,
}

/// Which ISA/mode the owned Unicorn instance executes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Backend {
    Arm,
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
}

impl Common {
    fn new(backend: Backend) -> Result<Self, EngineError> {
        let (arch, mode) = match backend {
            Backend::Arm => (Arch::ARM, Mode::ARM | Mode::LITTLE_ENDIAN),
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
        Ok(Self {
            backend,
            inner: OwnedUnicorn(uc),
            mappings: Vec::new(),
            mmio: Vec::new(),
            shared: Vec::new(),
            irq_edges: Vec::new(),
        })
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
        // The mirror is writable even when the guest may only read the
        // shared page: the synchronisation path writes it through the
        // loader API, which ignores guest permissions.
        self.inner
            .0
            .mem_map(base.0, size, Prot::READ | Prot::WRITE)
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
                .mem_write(base, &bytes)
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
            Backend::Arm => {
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
        // 32-bit backends only; PC writes must stay instruction-aligned. The
        // RV32I test firmware never uses the compressed extension, so 4-byte
        // alignment holds for both backends.
        let pc_id = match self.backend {
            Backend::Arm => 15,
            #[cfg(feature = "arch-riscv")]
            Backend::RiscV32 => 32,
        };
        if reg.0 == pc_id && value % 4 != 0 {
            return Err(EngineError::InvalidValue);
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

    fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError> {
        let supported = match self.backend {
            // ARMv7-A exposes the physical IRQ and FIQ inputs; any other
            // number is a composition error, not a guest fault.
            Backend::Arm => line.0 <= 1,
            // RV32I cores take the machine external interrupt on line 0.
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
        }
    };
}

unicorn_wrapper! {
    /// ARM A32 little-endian core adapter (ARM926EJ-S class).
    UnicornArm => Backend::Arm
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

    fn engine() -> UnicornArm {
        let mut engine = UnicornArm::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine
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
        // MOV r0,#0xAB; LDR r1,[pc,#0]; STRB r0,[r1]; .word 0x00010000
        first
            .write_memory(
                Addr(0x1000),
                &[
                    0xab, 0, 0xa0, 0xe3, 0, 0x10, 0x9f, 0xe5, 0, 0, 0xc1, 0xe5, 0, 0, 1, 0,
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
        assert_eq!(
            engine.reg_write(Register::Pc.into(), 0x1001),
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
