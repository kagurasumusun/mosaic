//! ARM Cortex-A53 CPU declaration (isa/aarch64, same placement as QEMU's
//! `target/arm` 64-bit half).
//!
//! Clean-room from the ARM Cortex-A53 TRM (DDI 0500) and ARMv8-A architecture
//! reference manual. This module owns only the ISA vocabulary + engine
//! configuration + the core's own exception input — the same split as
//! `cortex_m3` and `arm926ej_s`. The GIC CPU interface is a separate concern
//! (it lives in system registers Unicorn 2.1.x does not expose; ADR 0009
//! tracks the injection constraint), so this model exposes no MMIO window of
//! its own and does not synthesise CP15 system registers.
//!
//! `model` is the backend's CPU-model selection hint. Unicorn's AArch64
//! backend picks `cortex-a53` when asked; the engine's own caps decide
//! whether external IRQ injection is possible (ADR 0009: today it is not,
//! and the machine harness records the level without delivering).

use mosaic_contract::cpu_engine::{CpuDescriptor, EngineArch, IrqLine};

/// Execution configuration declaration: AArch64 little-endian, model
/// `cortex-a53`.
pub const CORE: CpuDescriptor = CpuDescriptor::new(EngineArch::AArch64Le, Some("cortex-a53"));

/// The core's physical IRQ input line. The GIC's Group-1 output drives this
/// line; Unicorn 2.1.x records the level without injecting an exception
/// (ADR 0009).
pub const IRQ_LINE: IrqLine = IrqLine(0);
/// The core's physical FIQ input line. The GIC's Group-0 output drives this
/// line.
pub const FIQ_LINE: IrqLine = IrqLine(1);

/// This core's ISA vocabulary (RegId is read through here).
pub use crate::Register;

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;

    #[test]
    fn the_descriptor_names_aarch64_and_pins_cortex_a53() {
        assert_eq!(CORE.arch, EngineArch::AArch64Le);
        assert_eq!(CORE.model, Some("cortex-a53"));
        assert_eq!(IRQ_LINE, IrqLine(0));
        assert_eq!(FIQ_LINE, IrqLine(1));
        assert_eq!(Register::X0 as u32, 0);
        assert_eq!(Register::Pc as u32, 32);
    }
}
