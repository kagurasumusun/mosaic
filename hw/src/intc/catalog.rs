//! Interrupt controllers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::intc::gic_v2::GicV2;
use crate::intc::ioapic::IoApic;
use crate::intc::pic_8259::Pic8259;
use crate::intc::pl190::{Pl190, Pl190Params};
use crate::intc::scs::{Scs, ScsParams};

/// Intel 8259A PIC: no IRQ outputs on this node (its vector is read by the
/// machine), inputs arrive as `irq` edges from devices.
fn spawn_pic_8259(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(Pic8259::new()))?;
    Ok(())
}

/// Intel I/O APIC: outputs through the machine's wiring; register model only.
fn spawn_ioapic(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(IoApic::new()))?;
    Ok(())
}

/// ARM GICv2 distributor: register model.
fn spawn_gic_v2(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(GicV2::new()))?;
    Ok(())
}

/// ARM PL190 VIC: `irq` and `fiq` outputs, 32 `inputs:n` IRQ inputs.
fn spawn_pl190(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let fiq = ctx.output("fiq")?;
    let vic = Pl190::new(Pl190Params::new(irq, fiq));
    machine.register_device(ctx.node, Box::new(vic))?;
    Ok(())
}

/// ARMv7-M System Control Space (NVIC + SysTick): `inputs:n` IRQ inputs,
/// parameters `num-irq`, `hclk-hz`.
fn spawn_scs(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let mut params = ScsParams::default();
    if let Some(num_irq) = ctx.parameter_int("num-irq") {
        if let Ok(num_irq) = usize::try_from(num_irq) {
            params.num_irq = num_irq;
        }
    }
    if let Some(hclk_hz) = ctx.parameter_int("hclk-hz") {
        if let Ok(hclk_hz) = u32::try_from(hclk_hz) {
            params.hclk_hz = hclk_hz;
        }
    }
    let mut scs = Scs::new(params);
    for wire in ctx.incoming {
        let Some(index) = wire.index() else {
            continue;
        };
        scs.bind_irq(index, wire.source, wire.signal)
            .map_err(|detail| MaterializeError::Bind {
                node: ctx.graph_id.to_owned(),
                detail,
            })?;
    }
    machine.register_device(ctx.node, Box::new(scs))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/gic-v2",
        summary: "ARM Generic Interrupt Controller v2 distributor (GICD register model)",
        outputs: &[],
        spawn: spawn_gic_v2,
    },
    ModelEntry {
        id: "ip:arm/pl190",
        summary: "ARM PrimeCell vectored interrupt controller (PL190)",
        outputs: &[],
        spawn: spawn_pl190,
    },
    ModelEntry {
        id: "ip:arm/scs",
        summary: "ARMv7-M System Control Space: NVIC, SysTick (SCS)",
        outputs: &[],
        spawn: spawn_scs,
    },
    ModelEntry {
        id: "ip:x86/pic-8259",
        summary: "Intel 8259A programmable interrupt controller (PC PIC, cascaded)",
        outputs: &[],
        spawn: spawn_pic_8259,
    },
    ModelEntry {
        id: "ip:x86/ioapic",
        summary: "Intel I/O APIC (IA-PC redirection table register model)",
        outputs: &[],
        spawn: spawn_ioapic,
    },
];
