#![forbid(unsafe_code)]
//! Native ARMv7-M execution core.
//!
//! Thumb-1 plus the 32-bit BL form, with precise faults and a real exception
//! stack frame. Unknown Thumb-2 encodings fault instead of being guessed.
//! The core does not know SoC addresses: MMIO leaves the engine through
//! [`EngineBus`], and NVIC prioritisation stays in `ip:arm/scs` (ADR 0017).

mod exec;

use core::num::NonZeroU64;

use mosaic_contract::cpu_engine::{
    CpuEngine, EngineBus, EngineBusFault, EngineCaps, EngineError, IrqLine, RunOutcome, StopReason,
};
use mosaic_exec::{AccessSize, Addr, BusFault, Permissions, RegId};

use exec::{enter_exception, step, ExecFault, ExecStop, Guest, GuestMem, MemFault};

/// Maskable exception input. ARMv7-M has no FIQ pin (ADR 0012).
pub const EXTERNAL_IRQ: IrqLine = IrqLine(0);

/// Functional Cortex-M3-class core. One instance is one core.
pub struct NativeCortexM3 {
    guest: Guest,
    regions: Vec<Region>,
    bus: Option<Box<dyn EngineBus>>,
    bus_fault: Option<EngineBusFault>,
    line_edges: Vec<(IrqLine, bool)>,
}

struct Region {
    base: u64,
    size: u64,
    perms: Permissions,
    kind: RegionKind,
}

enum RegionKind {
    Ram(Vec<u8>),
    Mmio,
}

struct MemPort<'a> {
    regions: &'a mut [Region],
    bus: &'a mut Option<Box<dyn EngineBus>>,
}

impl NativeCortexM3 {
    pub fn new() -> Self {
        Self {
            guest: Guest::new(),
            regions: Vec::new(),
            bus: None,
            bus_fault: None,
            line_edges: Vec::new(),
        }
    }

    pub fn attach_bus(&mut self, bus: Box<dyn EngineBus>) -> Result<(), EngineError> {
        if self.bus.is_some() {
            return Err(EngineError::BusAlreadyAttached);
        }
        self.bus = Some(bus);
        Ok(())
    }

    /// Callback window. A guest access is one bus transaction of the instruction
    /// width; unmapped holes stay unmapped.
    pub fn map_mmio(&mut self, base: Addr, size: u64) -> Result<(), EngineError> {
        self.insert(base, size, Permissions::RW, RegionKind::Mmio)
    }

    /// Vector-table base used by [`Self::enter_exception`]. Reset value is 0.
    pub fn set_vtor(&mut self, vtor: u32) {
        self.guest.vtor = vtor;
    }

    pub fn vtor(&self) -> u32 {
        self.guest.vtor
    }

    /// Take exception `number` at the current instruction boundary.
    ///
    /// The number is a composition-root input (SCS `VECTPENDING`), not
    /// something this core invents from a SoC address. Line 0 being high does
    /// not by itself choose a vector (ADR 0009 / 0012).
    pub fn enter_exception(&mut self, number: u32) -> Result<(), EngineError> {
        let NativeCortexM3 {
            guest,
            regions,
            bus,
            bus_fault,
            ..
        } = self;
        let mut port = MemPort { regions, bus };
        match enter_exception(guest, &mut port, number) {
            Ok(()) => Ok(()),
            Err(fault) => {
                note_fault(bus_fault, fault);
                Err(EngineError::Backend)
            }
        }
    }

    fn insert(
        &mut self,
        base: Addr,
        size: u64,
        perms: Permissions,
        kind: RegionKind,
    ) -> Result<(), EngineError> {
        if size == 0 || base.0.checked_add(size).is_none() {
            return Err(EngineError::InvalidMapping);
        }
        let end = base.0 + size;
        for region in &self.regions {
            let region_end = region.base + region.size;
            if base.0 < region_end && region.base < end {
                return Err(EngineError::Overlap);
            }
        }
        self.regions.push(Region {
            base: base.0,
            size,
            perms,
            kind,
        });
        self.regions.sort_by_key(|region| region.base);
        Ok(())
    }
}

impl Default for NativeCortexM3 {
    fn default() -> Self {
        Self::new()
    }
}

impl GuestMem for MemPort<'_> {
    fn load(&mut self, addr: u32, size: u8, execute: bool) -> Result<u32, MemFault> {
        transfer(self, u64::from(addr), size, None, execute)
    }

    fn store(&mut self, addr: u32, size: u8, value: u32) -> Result<(), MemFault> {
        transfer(self, u64::from(addr), size, Some(value), false).map(|_| ())
    }
}

fn transfer(
    port: &mut MemPort<'_>,
    addr: u64,
    size: u8,
    store: Option<u32>,
    execute: bool,
) -> Result<u32, MemFault> {
    let width = AccessSize::try_from(size).map_err(|_| MemFault {
        address: addr,
        width: size,
        fault: BusFault::InvalidSize,
    })?;
    let end = addr.checked_add(u64::from(size) - 1).ok_or(MemFault {
        address: addr,
        width: size,
        fault: BusFault::Unmapped,
    })?;
    if let Some(region) = region_at(port.regions, addr) {
        if matches!(region.kind, RegionKind::Mmio) && end < region.base + region.size {
            return mmio(port, addr, width, store);
        }
    }
    let mut assembled = 0u32;
    for byte_index in 0..size {
        let byte_addr = addr + u64::from(byte_index);
        let byte = match store {
            Some(value) => {
                let byte = (value >> (u32::from(byte_index) * 8)) as u8;
                write_ram(port, byte_addr, byte)?;
                byte
            }
            None => read_ram(port, byte_addr, execute)?,
        };
        assembled |= u32::from(byte) << (u32::from(byte_index) * 8);
    }
    Ok(assembled)
}

fn region_at(regions: &[Region], addr: u64) -> Option<&Region> {
    regions
        .iter()
        .find(|region| addr >= region.base && addr < region.base + region.size)
}

fn read_ram(port: &mut MemPort<'_>, addr: u64, execute: bool) -> Result<u8, MemFault> {
    let Some(region) = region_at(port.regions, addr) else {
        return Err(unmapped(addr));
    };
    if matches!(region.kind, RegionKind::Mmio) {
        return Err(unmapped(addr));
    }
    let allowed = if execute {
        region.perms.execute
    } else {
        region.perms.read
    };
    if !allowed {
        return Err(denied(addr));
    }
    let offset = (addr - region.base) as usize;
    match &region.kind {
        RegionKind::Ram(bytes) => Ok(bytes[offset]),
        RegionKind::Mmio => Err(unmapped(addr)),
    }
}

fn write_ram(port: &mut MemPort<'_>, addr: u64, value: u8) -> Result<(), MemFault> {
    let Some(index) = port
        .regions
        .iter()
        .position(|region| addr >= region.base && addr < region.base + region.size)
    else {
        return Err(unmapped(addr));
    };
    if !port.regions[index].perms.write || matches!(port.regions[index].kind, RegionKind::Mmio) {
        return Err(if matches!(port.regions[index].kind, RegionKind::Mmio) {
            unmapped(addr)
        } else {
            denied(addr)
        });
    }
    let offset = (addr - port.regions[index].base) as usize;
    if let RegionKind::Ram(bytes) = &mut port.regions[index].kind {
        bytes[offset] = value;
    }
    Ok(())
}

fn mmio(
    port: &mut MemPort<'_>,
    addr: u64,
    width: AccessSize,
    store: Option<u32>,
) -> Result<u32, MemFault> {
    let Some(region) = region_at(port.regions, addr) else {
        return Err(unmapped(addr));
    };
    let permitted = if store.is_some() {
        region.perms.write
    } else {
        region.perms.read
    };
    if !permitted {
        return Err(denied(addr));
    }
    let Some(bus) = port.bus.as_mut() else {
        return Err(unmapped(addr));
    };
    let address = Addr(addr);
    let width_byte = width.bytes();
    let outcome = if let Some(value) = store {
        bus.write(address, width, u64::from(value)).map(|()| value)
    } else {
        bus.read(address, width).map(|value| value as u32)
    };
    outcome.map_err(|fault| MemFault {
        address: addr,
        width: width_byte,
        fault,
    })
}

fn unmapped(addr: u64) -> MemFault {
    MemFault {
        address: addr,
        width: 1,
        fault: BusFault::Unmapped,
    }
}

fn denied(addr: u64) -> MemFault {
    MemFault {
        address: addr,
        width: 1,
        fault: BusFault::PermissionDenied,
    }
}

fn note_fault(slot: &mut Option<EngineBusFault>, fault: ExecFault) {
    if let ExecFault::Mem(MemFault {
        address,
        width,
        fault,
    }) = fault
    {
        *slot = Some(EngineBusFault::new(Addr(address), width, fault));
    }
}

impl CpuEngine for NativeCortexM3 {
    fn caps(&self) -> EngineCaps {
        // Line 0 is the wake/pending input. The vector number is not on the
        // line, so set_line does not enter an exception by itself.
        EngineCaps::PRECISE
    }

    fn run(&mut self, instructions: NonZeroU64) -> RunOutcome {
        let budget = instructions.get();
        let mut hooks = 0u64;
        while hooks < budget {
            hooks += 1;
            let NativeCortexM3 {
                guest,
                regions,
                bus,
                bus_fault,
                ..
            } = self;
            let mut port = MemPort { regions, bus };
            match step(guest, &mut port) {
                Ok(None) => {}
                Ok(Some(ExecStop::WaitingForInterrupt)) => {
                    return RunOutcome {
                        reason: StopReason::WaitingForInterrupt,
                        instruction_hooks: hooks,
                    };
                }
                Ok(Some(ExecStop::Breakpoint)) => {
                    return RunOutcome {
                        reason: StopReason::Breakpoint,
                        instruction_hooks: hooks,
                    };
                }
                Err(fault) => {
                    note_fault(bus_fault, fault);
                    return RunOutcome {
                        reason: StopReason::Fault,
                        instruction_hooks: hooks,
                    };
                }
            }
        }
        RunOutcome {
            reason: StopReason::BudgetExhausted,
            instruction_hooks: hooks,
        }
    }

    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError> {
        self.guest
            .r
            .get(usize::try_from(reg.0).map_err(|_| EngineError::InvalidRegister)?)
            .copied()
            .map(u64::from)
            .ok_or(EngineError::InvalidRegister)
    }

    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError> {
        let index = usize::try_from(reg.0).map_err(|_| EngineError::InvalidRegister)?;
        if index >= self.guest.r.len() {
            return Err(EngineError::InvalidRegister);
        }
        let value = u32::try_from(value & 0xffff_ffff).map_err(|_| EngineError::InvalidValue)?;
        if index == 15 && value & 1 != 0 {
            return Err(EngineError::InvalidValue);
        }
        self.guest.r[index] = if index == 13 { value & !0b11 } else { value };
        Ok(())
    }

    fn map_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
    ) -> Result<(), EngineError> {
        let len = usize::try_from(size).map_err(|_| EngineError::InvalidMapping)?;
        self.insert(base, size, permissions, RegionKind::Ram(vec![0; len]))
    }

    fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError> {
        if line != EXTERNAL_IRQ {
            return Err(EngineError::LineNotSupported);
        }
        self.line_edges.push((line, high));
        self.guest.irq_line = high;
        Ok(())
    }

    fn write_memory(&mut self, base: Addr, data: &[u8]) -> Result<(), EngineError> {
        for (index, byte) in data.iter().enumerate() {
            let addr = base
                .0
                .checked_add(u64::try_from(index).map_err(|_| EngineError::InvalidMapping)?)
                .ok_or(EngineError::InvalidMapping)?;
            let region = self
                .regions
                .iter_mut()
                .find(|region| addr >= region.base && addr < region.base + region.size)
                .ok_or(EngineError::Unmapped)?;
            match &mut region.kind {
                RegionKind::Ram(bytes) => {
                    let offset =
                        usize::try_from(addr - region.base).map_err(|_| EngineError::Unmapped)?;
                    bytes[offset] = *byte;
                }
                RegionKind::Mmio => return Err(EngineError::InvalidMapping),
            }
        }
        Ok(())
    }

    fn read_memory(&self, base: Addr, data: &mut [u8]) -> Result<(), EngineError> {
        for (index, slot) in data.iter_mut().enumerate() {
            let addr = base
                .0
                .checked_add(u64::try_from(index).map_err(|_| EngineError::InvalidMapping)?)
                .ok_or(EngineError::InvalidMapping)?;
            let region = self
                .regions
                .iter()
                .find(|region| addr >= region.base && addr < region.base + region.size)
                .ok_or(EngineError::Unmapped)?;
            match &region.kind {
                RegionKind::Ram(bytes) => {
                    let offset =
                        usize::try_from(addr - region.base).map_err(|_| EngineError::Unmapped)?;
                    *slot = bytes[offset];
                }
                RegionKind::Mmio => return Err(EngineError::InvalidMapping),
            }
        }
        Ok(())
    }

    fn take_bus_fault(&mut self) -> Option<EngineBusFault> {
        self.bus_fault.take()
    }

    fn line_edges(&self) -> &[(IrqLine, bool)] {
        &self.line_edges
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use mosaic_contract::cpu_engine::{CpuEngine, EngineBus, EngineError, StopReason};
    use mosaic_exec::{AccessSize, Addr, BusFault, Permissions};
    use mosaic_isa_arm::Register;

    use super::*;

    fn engine(code: &[u8]) -> NativeCortexM3 {
        let mut engine = NativeCortexM3::new();
        engine
            .map_memory(Addr(0), 0x1000, Permissions::RWX)
            .unwrap();
        engine.write_memory(Addr(0), code).unwrap();
        engine.reg_write(Register::Pc.into(), 0).unwrap();
        engine
    }

    fn run(engine: &mut NativeCortexM3, n: u64) -> RunOutcome {
        engine.run(NonZeroU64::new(n).unwrap())
    }

    #[test]
    fn sxth_and_rev_use_the_opcode_byte_not_a_dead_mask() {
        // SXTH r0, r1; REV r2, r1
        let mut cpu = engine(&[0x08, 0xB2, 0x0A, 0xBA]);
        cpu.reg_write(Register::R1.into(), 0x80FF_00A5).unwrap();
        run(&mut cpu, 2);
        assert_eq!(cpu.reg_read(Register::R0.into()).unwrap(), 0xA5);
        assert_eq!(cpu.reg_read(Register::R2.into()).unwrap(), 0xA500_FF80);
    }

    #[test]
    fn hex_nibble_extract_matches_the_firmware_sequence() {
        // LSLS r1, r0, #28; LSRS r0, r0, #4; LSRS r1, r1, #28; CMP r1, #10
        let mut cpu = engine(&[0x01, 0x07, 0x00, 0x09, 0x09, 0x0F, 0x0A, 0x29]);
        cpu.reg_write(Register::R0.into(), 0xC0).unwrap();
        run(&mut cpu, 4);
        // Low nibble of 0xC0 is 0; the remaining word is 0xC. CMP #10 borrows.
        assert_eq!(cpu.reg_read(Register::R1.into()).unwrap(), 0);
        assert_eq!(cpu.reg_read(Register::R0.into()).unwrap(), 0xC);
        assert!(!cpu.guest.c);
        assert!(cpu.guest.n);
    }

    #[test]
    fn adds_and_cmp_set_architectural_flags() {
        // MOVS r0, #1; MOVS r1, #2; ADDS r0, r0, r1; CMP r0, #10; BLO .
        let mut cpu = engine(&[0x01, 0x20, 0x02, 0x21, 0x40, 0x18, 0x0A, 0x28, 0xFE, 0xD3]);
        let outcome = run(&mut cpu, 4);
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(cpu.reg_read(Register::R0.into()).unwrap(), 3);
        assert!(!cpu.guest.z);
        assert!(!cpu.guest.c);
        let branch = run(&mut cpu, 1);
        assert_eq!(branch.reason, StopReason::BudgetExhausted);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 8);
    }

    #[test]
    fn bl_matches_the_t32_immediate_formula() {
        let mut image = vec![0u8; 0x140];
        image[0x32] = 0x00;
        image[0x33] = 0xF0;
        image[0x34] = 0x6E;
        image[0x35] = 0xF8;
        image[0x130] = 0xFF;
        image[0x131] = 0xF7;
        image[0x132] = 0xEF;
        image[0x133] = 0xFF;
        let mut cpu = engine(&image);
        cpu.reg_write(Register::Pc.into(), 0x32).unwrap();
        assert_eq!(run(&mut cpu, 1).reason, StopReason::BudgetExhausted);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0x112);
        assert_eq!(cpu.reg_read(Register::Lr.into()).unwrap(), 0x37);
        cpu.reg_write(Register::Pc.into(), 0x130).unwrap();
        run(&mut cpu, 1);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0x112);
        assert_eq!(cpu.reg_read(Register::Lr.into()).unwrap(), 0x135);
    }

    #[test]
    fn push_blx_and_pop_pc_roundtrip() {
        // PUSH {LR}; BLX r1; at the target: POP {PC}. r1 = 0x21.
        let mut cpu = engine(&[0x00, 0xB5, 0x88, 0x47, 0x00, 0xBD]);
        cpu.reg_write(Register::R1.into(), 0x5).unwrap();
        cpu.reg_write(Register::Sp.into(), 0x200).unwrap();
        cpu.reg_write(Register::Lr.into(), 0x11).unwrap();
        run(&mut cpu, 3);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0x10);
        assert_eq!(cpu.reg_read(Register::Sp.into()).unwrap(), 0x200);
    }

    #[test]
    fn it_eq_skips_the_following_add_when_z_is_clear() {
        // MOVS r0, #1; IT EQ; ADDS r0, #1
        let mut cpu = engine(&[0x01, 0x20, 0x08, 0xBF, 0x01, 0x30]);
        run(&mut cpu, 3);
        assert_eq!(cpu.reg_read(Register::R0.into()).unwrap(), 1);
        assert_eq!(cpu.guest.it, 0);
    }

    #[test]
    fn unknown_wide_encoding_faults_without_retiring() {
        let mut cpu = engine(&[0x00, 0xF0, 0x00, 0x00]);
        let outcome = run(&mut cpu, 4);
        assert_eq!(outcome.reason, StopReason::Fault);
        assert_eq!(outcome.instruction_hooks, 1);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0);
        assert!(cpu.take_bus_fault().is_none());
    }

    #[test]
    fn mmio_word_is_one_bus_transaction() {
        struct Log(Vec<(bool, u64, u8, u64)>);
        impl EngineBus for Log {
            fn read(&mut self, address: Addr, width: AccessSize) -> Result<u64, BusFault> {
                self.0.push((false, address.0, width.bytes(), 0));
                Ok(0x1122_3344)
            }
            fn write(
                &mut self,
                address: Addr,
                width: AccessSize,
                value: u64,
            ) -> Result<(), BusFault> {
                self.0.push((true, address.0, width.bytes(), value));
                Ok(())
            }
        }
        let log = std::sync::Arc::new(std::sync::Mutex::new(Log(Vec::new())));
        struct Shared(std::sync::Arc<std::sync::Mutex<Log>>);
        impl EngineBus for Shared {
            fn read(&mut self, address: Addr, width: AccessSize) -> Result<u64, BusFault> {
                self.0.lock().unwrap().read(address, width)
            }
            fn write(
                &mut self,
                address: Addr,
                width: AccessSize,
                value: u64,
            ) -> Result<(), BusFault> {
                self.0.lock().unwrap().write(address, width, value)
            }
        }
        // LDR r1, [r0]; STR r1, [r0]
        let mut cpu = engine(&[0x01, 0x68, 0x01, 0x60]);
        cpu.attach_bus(Box::new(Shared(std::sync::Arc::clone(&log))))
            .unwrap();
        cpu.map_mmio(Addr(0x4000_0000), 0x100).unwrap();
        cpu.reg_write(Register::R0.into(), 0x4000_0000).unwrap();
        cpu.reg_write(Register::R1.into(), 0x55).unwrap();
        run(&mut cpu, 2);
        assert_eq!(cpu.reg_read(Register::R1.into()).unwrap(), 0x1122_3344);
        let log = log.lock().unwrap();
        assert_eq!(
            log.0,
            vec![
                (false, 0x4000_0000, 4, 0),
                (true, 0x4000_0000, 4, 0x1122_3344),
            ]
        );
    }

    #[test]
    fn partitioned_budget_matches_one_run() {
        let code = [0x01, 0x20, 0x02, 0x21, 0x40, 0x18];
        let mut whole = engine(&code);
        let mut split = engine(&code);
        run(&mut whole, 3);
        run(&mut split, 1);
        run(&mut split, 2);
        for reg in [Register::R0, Register::R1, Register::Pc] {
            assert_eq!(
                whole.reg_read(reg.into()).unwrap(),
                split.reg_read(reg.into()).unwrap()
            );
        }
    }

    #[test]
    fn wfi_sleeps_until_the_external_line_is_high() {
        // WFI; MOVS r0, #1
        let mut cpu = engine(&[0x30, 0xBF, 0x01, 0x20]);
        let slept = run(&mut cpu, 4);
        assert_eq!(slept.reason, StopReason::WaitingForInterrupt);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0);
        cpu.set_line(EXTERNAL_IRQ, true).unwrap();
        let woke = run(&mut cpu, 2);
        assert_eq!(woke.reason, StopReason::BudgetExhausted);
        assert_eq!(cpu.reg_read(Register::R0.into()).unwrap(), 1);
        assert!(cpu.set_line(IrqLine(1), true).is_err());
    }

    #[test]
    fn exception_entry_stacks_and_return_restores() {
        let mut cpu = NativeCortexM3::new();
        cpu.map_memory(Addr(0), 0x1000, Permissions::RWX).unwrap();
        // BX LR at the handler.
        cpu.write_memory(Addr(0x80), &[0x70, 0x47]).unwrap();
        cpu.write_memory(Addr(16 * 4), &0x81u32.to_le_bytes())
            .unwrap();
        cpu.reg_write(Register::Sp.into(), 0x200).unwrap();
        cpu.reg_write(Register::R0.into(), 0x42).unwrap();
        cpu.reg_write(Register::Pc.into(), 0x10).unwrap();
        cpu.enter_exception(16).unwrap();
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0x80);
        assert_eq!(cpu.reg_read(Register::Lr.into()).unwrap(), 0xFFFF_FFF9);
        assert_eq!(cpu.reg_read(Register::Sp.into()).unwrap(), 0x1E0);
        run(&mut cpu, 1);
        assert_eq!(cpu.reg_read(Register::R0.into()).unwrap(), 0x42);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0x10);
        assert_eq!(cpu.reg_read(Register::Sp.into()).unwrap(), 0x200);
    }

    #[test]
    fn store_to_execute_only_memory_is_a_permission_fault() {
        let mut cpu = NativeCortexM3::new();
        cpu.map_memory(Addr(0), 0x100, Permissions::RX).unwrap();
        // STR r0, [r1] with r1 = 0.
        cpu.write_memory(Addr(0), &[0x08, 0x60]).unwrap();
        let outcome = run(&mut cpu, 1);
        assert_eq!(outcome.reason, StopReason::Fault);
        assert_eq!(cpu.reg_read(Register::Pc.into()).unwrap(), 0);
        let fault = cpu.take_bus_fault().unwrap();
        assert_eq!(fault.fault, BusFault::PermissionDenied);
        let _ = EngineError::Backend;
    }
}
