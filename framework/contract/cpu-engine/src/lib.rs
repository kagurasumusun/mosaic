#![no_std]
#![forbid(unsafe_code)]
//! Engine-neutral, synchronous single-core execution. No vendor types escape.

use core::num::NonZeroU64;
use mosaic_core_types::{AccessSize, Addr, BusFault, Permissions};

pub use mosaic_core_types::RegId;

/// A guest bus access the engine could not complete through its callback port.
///
/// `width` is the raw transfer width the backend requested; it may be a value
/// outside the [`AccessSize`] vocabulary, in which case `fault` is
/// [`BusFault::InvalidSize`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct EngineBusFault {
    pub address: Addr,
    pub width: u8,
    pub fault: BusFault,
}

impl EngineBusFault {
    /// Constructor for engine backends, which cannot name the fields of a
    /// `non_exhaustive` struct from another crate.
    pub fn new(address: Addr, width: u8, fault: BusFault) -> Self {
        Self {
            address,
            width,
            fault,
        }
    }
}

/// Engine-facing callback bus for MMIO regions.
///
/// The engine knows guest addresses and transfer widths only; the concrete L5
/// binding resolves them through the runtime FlatView and synthesizes the bus
/// [`mosaic_core_types::Attr`] for the engine master. Engines never depend on
/// runtime types, so the port stays in the engine contract.
pub trait EngineBus: Send {
    fn read(&mut self, address: Addr, width: AccessSize) -> Result<u64, BusFault>;
    fn write(&mut self, address: Addr, width: AccessSize, value: u64) -> Result<(), BusFault>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct EngineCaps {
    pub instruction_count: bool,
    pub precise_exceptions: bool,
    pub exclusive_monitor: bool,
    /// Whether [`CpuEngine::set_line`] reaches the guest's exception input.
    /// When false, line levels are recorded but cannot preempt guest code.
    pub line_injection: bool,
}

impl EngineCaps {
    /// Functional instruction budgets; not precise exception or SMP guarantees.
    pub const COUNTED: Self = Self {
        instruction_count: true,
        precise_exceptions: false,
        exclusive_monitor: false,
        line_injection: false,
    };

    /// Instruction budgets plus precise faults. Still no exclusive monitor and
    /// no line-driven exception entry (the vector number is not on the line).
    pub const PRECISE: Self = Self {
        instruction_count: true,
        precise_exceptions: true,
        exclusive_monitor: false,
        line_injection: false,
    };
}

/// Architecture-neutral execution architecture vocabulary for CPU descriptors
/// (design §12.1: `isa` names the vocabulary, the engine backend is chosen by
/// the composition root). Values name ISA + mode combinations, never vendors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EngineArch {
    /// ARM A32 (ARMv4/v5 little-endian, ARM926EJ-S class cores).
    ArmA32Le,
    /// ARM Thumb-2 / ARMv7-M little-endian (Cortex-M3 class cores).
    /// The M-class engine configuration is ADR 0012; this variant only names it.
    ArmV7mLe,
    /// RISC-V RV32 base integer ISA, little-endian.
    RiscV32Le,
}

/// Catalog declaration of one processor core's execution configuration
/// (design §12.1: cpu = ISA + engine settings + core-local blocks).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuDescriptor {
    pub arch: EngineArch,
    /// Backend CPU-model hint (for example `"sifive-e31"`). A backend without
    /// model selection ignores it; the declaration stays the source of truth.
    pub model: Option<&'static str>,
}

impl CpuDescriptor {
    /// Constructor for catalog crates: the struct stays open for future
    /// fields without breaking existing declarations.
    pub const fn new(arch: EngineArch, model: Option<&'static str>) -> Self {
        Self { arch, model }
    }
}

/// Architecture-neutral logical interrupt line. Numbering is owned by the ISA
/// vocabulary or the board composition; the contract assigns no ARM-specific
/// meaning to any value (see ADR 0009).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct IrqLine(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EngineError {
    InvalidRegister,
    InvalidValue,
    InvalidMapping,
    Overlap,
    Unmapped,
    Backend,
    BusNotAttached,
    BusAlreadyAttached,
    /// The engine does not accept this interrupt line, for example an unknown
    /// line number on a single-core ARM adapter.
    LineNotSupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StopReason {
    BudgetExhausted,
    WaitingForInterrupt,
    Halted,
    Breakpoint,
    CapabilityMissing,
    Fault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    pub reason: StopReason,
    /// Instruction hooks observed. On Fault this is not a retired-instruction
    /// count: the last instruction may have failed after its pre-execution hook.
    pub instruction_hooks: u64,
}

pub trait CpuEngine: Send {
    fn caps(&self) -> EngineCaps;
    fn run(&mut self, instructions: NonZeroU64) -> RunOutcome;
    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError>;
    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError>;
    /// Initial private-memory path. A future AddressView/DMI binding replaces
    /// this without exposing raw backend handles or host pointers to models.
    fn map_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
    ) -> Result<(), EngineError>;
    /// Drive one logical interrupt line (see ADR 0009). Delivery happens only
    /// between run quanta; an engine that cannot inject external interrupts
    /// records the level and reports `EngineCaps::line_injection == false`.
    fn set_line(&mut self, line: IrqLine, high: bool) -> Result<(), EngineError>;
    /// Loader/debug access, not guest access: writes may initialize ROM and code
    /// pages. Patching executable bytes must invalidate translated code.
    fn write_memory(&mut self, base: Addr, data: &[u8]) -> Result<(), EngineError>;
    fn read_memory(&self, base: Addr, data: &mut [u8]) -> Result<(), EngineError>;
    /// Take and clear the first callback-bus fault recorded since the previous
    /// call (ADR 0010). Engines without a callback bus keep the default `None`.
    fn take_bus_fault(&mut self) -> Option<EngineBusFault> {
        None
    }
    /// `(line, high)` edges recorded through [`CpuEngine::set_line`], in FIFO
    /// order. Observable for wiring tests while `line_injection` is false;
    /// engines that do not record keep the default empty log (ADR 0010).
    fn line_edges(&self) -> &[(IrqLine, bool)] {
        &[]
    }
    /// Copy the engine's shared-memory mirrors from the authoritative buffers
    /// (before a core slice) and back into them (after a core slice).
    ///
    /// Backends that map one writable shared memory in every instance can
    /// leave the default no-op. Backends whose host mapping is read-only
    /// towards the guest (Unicorn 2.1.x `uc_mem_map_ptr`) implement the
    /// copy at deterministic slice boundaries instead (ADR 0010).
    fn sync_shared_in(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
    fn sync_shared_out(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
}
