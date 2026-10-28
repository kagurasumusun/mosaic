//! Interrupt controller IP blocks (QEMU `hw/intc/`).

use crate::catalog::ModelEntry;
use crate::ip::intc::aia::Aia;
use crate::ip::intc::clint::Clint;
use crate::ip::intc::exti_f1::ExtiF1;
use crate::ip::intc::exti_f4::ExtiF4;
use crate::ip::intc::exti_h7::ExtiH7;
use crate::ip::intc::gic_v2::GicV2;
use crate::ip::intc::gic_v3::GicV3;
use crate::ip::intc::gic_v4::GicV4;
use crate::ip::intc::i8259_pic::I8259Pic;
use crate::ip::intc::ioapic::IoApic;
use crate::ip::intc::lapic::Lapic;
use crate::ip::intc::nvic::Nvic;
use crate::ip::intc::nvic_f1::NvicF1;
use crate::ip::intc::pic_8259::Pic8259;
use crate::ip::intc::pl190::{Pl190, Pl190Params};
use crate::ip::intc::pl192::Pl192;
use crate::ip::intc::plic::Plic;
use crate::ip::intc::rp2040_intc::Rp2040Intc;
use crate::ip::intc::scs::{Scs, ScsParams};
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

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
/// Default spawn for i8259-pic: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i8259_pic(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I8259Pic::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lapic: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lapic(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lapic::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for gic-v3: parameters select the SPI and redistributor
/// counts; `irq` (if wired) is cpu 0's Group-1 output and `fiq` cpu 0's
/// Group-0 output. Device IRQ outputs route into the GIC by INTID through
/// signals.
fn spawn_gic_v3(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let params = crate::ip::intc::gic_v3::GicV3Params {
        spis: ctx.parameter_int("spis").unwrap_or(224) as u32,
        cpus: ctx.parameter_int("redistributors").unwrap_or(1) as u32,
    };
    let mut device = GicV3::new(params);
    if let Ok(signal) = ctx.output("irq") {
        device.bind_irq_output(0, signal);
    }
    if let Ok(signal) = ctx.output("fiq") {
        device.bind_fiq_output(0, signal);
    }
    machine.register_snapshot_device(ctx.node, device)?;
    Ok(())
}
/// Default spawn for gic-v4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_gic_v4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = GicV4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for nvic: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_nvic(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Nvic::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pl192: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pl192(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pl192::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for plic: parameters select the source/context counts;
/// the output table's `irq` entry (source-ID-addressed lines come through
/// signals) wires context 0's external-interrupt line.
fn spawn_plic(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let params = crate::ip::intc::plic::PlicParams {
        sources: ctx.parameter_int("sources").unwrap_or(96) as u32,
        contexts: ctx.parameter_int("contexts").unwrap_or(1) as u32,
    };
    let mut device = Plic::new(params);
    if let Ok(signal) = ctx.output("irq") {
        device.bind_output(0, signal);
    }
    machine.register_snapshot_device(ctx.node, device)?;
    Ok(())
}
/// Default spawn for clint: parameters select the hart count and tick
/// frequency; `irq` (if wired) is hart 0's machine timer line and `msip`
/// is hart 0's software-interrupt line.
fn spawn_clint(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let params = crate::ip::intc::clint::ClintParams {
        harts: ctx.parameter_int("harts").unwrap_or(1) as u32,
        hz: ctx.parameter_int("hz").unwrap_or(10_000_000) as u64,
    };
    let mut device = Clint::new(params);
    if let Ok(signal) = ctx.output("irq") {
        device.bind_mtip_output(0, signal);
    }
    if let Ok(signal) = ctx.output("msip") {
        device.bind_msip_output(0, signal);
    }
    machine.register_snapshot_device(ctx.node, device)?;
    Ok(())
}
/// Default spawn for aia: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_aia(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Aia::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for nvic-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_nvic_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = NvicF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for exti-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_exti_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = ExtiF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for exti-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_exti_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = ExtiF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for exti-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_exti_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = ExtiH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rp2040-intc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_intc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Intc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
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
    ModelEntry {
        id: "ip:intel/i8259-pic",
        summary: "Intel 8259A Programmable Interrupt Controller",
        outputs: &[],
        spawn: spawn_i8259_pic,
    },
    ModelEntry {
        id: "ip:intel/lapic",
        summary: "Intel Local APIC",
        outputs: &[],
        spawn: spawn_lapic,
    },
    ModelEntry {
        id: "ip:arm/gic-v3",
        summary: "ARM Generic Interrupt Controller v3",
        outputs: &[],
        spawn: spawn_gic_v3,
    },
    ModelEntry {
        id: "ip:arm/gic-v4",
        summary: "ARM Generic Interrupt Controller v4",
        outputs: &[],
        spawn: spawn_gic_v4,
    },
    ModelEntry {
        id: "ip:arm/nvic",
        summary: "ARM Nested Vectored Interrupt Controller",
        outputs: &[],
        spawn: spawn_nvic,
    },
    ModelEntry {
        id: "ip:arm/pl192",
        summary: "ARM PL192 Vectored Interrupt Controller",
        outputs: &[],
        spawn: spawn_pl192,
    },
    ModelEntry {
        id: "ip:riscv/plic",
        summary: "RISC-V Platform-Level Interrupt Controller",
        outputs: &[],
        spawn: spawn_plic,
    },
    ModelEntry {
        id: "ip:riscv/clint",
        summary: "RISC-V Core-Local Interruptor",
        outputs: &[],
        spawn: spawn_clint,
    },
    ModelEntry {
        id: "ip:riscv/aia",
        summary: "RISC-V Advanced Interrupt Architecture",
        outputs: &[],
        spawn: spawn_aia,
    },
    ModelEntry {
        id: "ip:st/nvic-f1",
        summary: "STM32F1 NVIC (ARM Cortex-M3)",
        outputs: &[],
        spawn: spawn_nvic_f1,
    },
    ModelEntry {
        id: "ip:st/exti-f1",
        summary: "STM32F1 EXTI",
        outputs: &[],
        spawn: spawn_exti_f1,
    },
    ModelEntry {
        id: "ip:st/exti-f4",
        summary: "STM32F4 EXTI",
        outputs: &[],
        spawn: spawn_exti_f4,
    },
    ModelEntry {
        id: "ip:st/exti-h7",
        summary: "STM32H7 EXTI",
        outputs: &[],
        spawn: spawn_exti_h7,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-intc",
        summary: "Raspberry Pi RP2040 interrupt controller",
        outputs: &[],
        spawn: spawn_rp2040_intc,
    },
];
