//! SiFive U54 core declaration (isa/riscv, same placement as QEMU's
//! `target/riscv`).
//!
//! U54 is an RV64GC application core (SiFive U540 / Freedom Unleashed). This
//! module owns the ISA + engine configuration declaration only — the same
//! split as `e31`. CLINT and PLIC are independent IP blocks the SoC
//! declaration composes; this core model exposes no MMIO window and does
//! not synthesise CSRs Unicorn 2.1.x does not expose.
//!
//! `model` is the backend's CPU-model selection hint. Unicorn 2.1.x's Rust
//! bindings do not publish CPU-model selection, so today the declaration is
//! authoritative but the backend runs its default RV64 core (ADR 0010).

use mosaic_contract::cpu_engine::{CpuDescriptor, EngineArch, IrqLine};

/// Execution configuration declaration: RV64 little-endian, model
/// `sifive-u54`.
pub const CORE: CpuDescriptor = CpuDescriptor::new(EngineArch::RiscV64Le, Some("sifive-u54"));

/// The core's machine-mode external interrupt input. The PLIC's
/// context-0 `meip` line drives this; Unicorn 2.1.x records the level
/// without injecting an exception (ADR 0009).
pub const M_EXT_LINE: IrqLine = IrqLine(0);
/// The core's machine-mode timer interrupt input. The CLINT's hart-0
/// `mtip` line drives this.
pub const M_TIMER_LINE: IrqLine = IrqLine(1);
/// The core's machine-mode software interrupt input. The CLINT's hart-0
/// `msip` line drives this.
pub const M_SOFT_LINE: IrqLine = IrqLine(2);

/// This core's ISA vocabulary (RegId is read through here).
pub use crate::Register;

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;

    #[test]
    fn the_descriptor_declares_rv64_and_the_u54_model() {
        assert_eq!(CORE.arch, EngineArch::RiscV64Le);
        assert_eq!(CORE.model, Some("sifive-u54"));
        assert_eq!(M_EXT_LINE, IrqLine(0));
        assert_eq!(M_TIMER_LINE, IrqLine(1));
        assert_eq!(M_SOFT_LINE, IrqLine(2));
        assert_eq!(Register::A0, Register::X10);
    }
}
