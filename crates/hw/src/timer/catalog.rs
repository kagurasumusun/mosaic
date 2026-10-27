//! Timers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::timer::am335x_timer::Am335XTimer;
use crate::timer::efm32_timer::Efm32Timer;
use crate::timer::i8253_pit::I8253Pit;
use crate::timer::i8254_pit::I8254Pit;
use crate::timer::i8259_pit::I8259Pit;
use crate::timer::kinetis_ftm::KinetisFtm;
use crate::timer::kinetis_lptmr::KinetisLptmr;
use crate::timer::kinetis_pit::KinetisPit;
use crate::timer::lpc11xx_timer::Lpc11XxTimer;
use crate::timer::lpc17xx_timer::Lpc17XxTimer;
use crate::timer::mtk_timer::MtkTimer;
use crate::timer::pic32_timer::Pic32Timer;
use crate::timer::pit_8254::Pit8254;
use crate::timer::rk_timer::RkTimer;
use crate::timer::rp2040_pwm::Rp2040Pwm;
use crate::timer::rp2040_timer::Rp2040Timer;
use crate::timer::rza1_timer::Rza1Timer;
use crate::timer::sp804::{Sp804, Sp804Params};
use crate::timer::sp805::Sp805;
use crate::timer::sunxi_timer::SunxiTimer;
use crate::timer::tim_f1::TimF1;
use crate::timer::tim_f4::TimF4;
use crate::timer::tim_h7::TimH7;
use crate::timer::tim_l4::TimL4;

/// Intel 8254 PIT: register model (the machine's timer wiring is its own).
fn spawn_pit_8254(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(Pit8254::new()))?;
    Ok(())
}

/// ARM Dual-Timer SP804: `timer1`/`timer2` IRQ outputs, `timclk` clock.
fn spawn_sp804(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let timer1 = ctx.output("timer1")?;
    let timer2 = ctx.output("timer2")?;
    let mut tick_ps = 1_000u64; // 1 ns
    if let Some(value) = ctx.parameter_int("tick-ps") {
        if value > 0 {
            tick_ps = value as u64;
        }
    }
    let Some(tick_ps) = std::num::NonZeroU64::new(tick_ps) else {
        return Err(MaterializeError::Bind {
            node: ctx.graph_id.to_owned(),
            detail: "tick-ps must be nonzero".into(),
        });
    };
    let timer = Sp804::new(Sp804Params::new([timer1, timer2], tick_ps));
    machine.register_device(ctx.node, Box::new(timer))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:x86/pit-8254",
        summary: "Intel 8254 programmable interval timer (PC PIT, 3 channels)",
        outputs: &[],
        spawn: spawn_pit_8254,
    },
    ModelEntry {
        id: "ip:arm/sp804",
        summary: "ARM Dual-Timer Module (SP804) event-driven functional model",
        outputs: &[],
        spawn: spawn_sp804,
    },
    ModelEntry {
        id: "ip:intel/i8253-pit",
        summary: "Intel 8253 Programmable Interval Timer",
        outputs: &[],
        spawn: spawn_i8253_pit,
    },
    ModelEntry {
        id: "ip:intel/i8254-pit",
        summary: "Intel 8254 Programmable Interval Timer",
        outputs: &[],
        spawn: spawn_i8254_pit,
    },
    ModelEntry {
        id: "ip:arm/sp805",
        summary: "ARM SP805 watchdog",
        outputs: &[],
        spawn: spawn_sp805,
    },
    ModelEntry {
        id: "ip:st/tim-f1",
        summary: "STM32F1 timer (RM0008)",
        outputs: &[],
        spawn: spawn_tim_f1,
    },
    ModelEntry {
        id: "ip:st/tim-f4",
        summary: "STM32F4 timer (RM0090)",
        outputs: &[],
        spawn: spawn_tim_f4,
    },
    ModelEntry {
        id: "ip:st/tim-h7",
        summary: "STM32H7 timer",
        outputs: &[],
        spawn: spawn_tim_h7,
    },
    ModelEntry {
        id: "ip:st/tim-l4",
        summary: "STM32L4 timer",
        outputs: &[],
        spawn: spawn_tim_l4,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-timer",
        summary: "NXP LPC11xx timer",
        outputs: &[],
        spawn: spawn_lpc11xx_timer,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-timer",
        summary: "NXP LPC17xx timer",
        outputs: &[],
        spawn: spawn_lpc17xx_timer,
    },
    ModelEntry {
        id: "ip:nxp/kinetis-pit",
        summary: "NXP Kinetis PIT",
        outputs: &[],
        spawn: spawn_kinetis_pit,
    },
    ModelEntry {
        id: "ip:nxp/kinetis-ftm",
        summary: "NXP Kinetis FTM",
        outputs: &[],
        spawn: spawn_kinetis_ftm,
    },
    ModelEntry {
        id: "ip:nxp/kinetis-lptmr",
        summary: "NXP Kinetis LPTMR",
        outputs: &[],
        spawn: spawn_kinetis_lptmr,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-timer",
        summary: "Raspberry Pi RP2040 timer",
        outputs: &[],
        spawn: spawn_rp2040_timer,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-pwm",
        summary: "Raspberry Pi RP2040 PWM slice",
        outputs: &[],
        spawn: spawn_rp2040_pwm,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-timer",
        summary: "Allwinner Sunxi timer",
        outputs: &[],
        spawn: spawn_sunxi_timer,
    },
    ModelEntry {
        id: "ip:rockchip/rk-timer",
        summary: "Rockchip timer",
        outputs: &[],
        spawn: spawn_rk_timer,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-timer",
        summary: "MediaTek timer",
        outputs: &[],
        spawn: spawn_mtk_timer,
    },
    ModelEntry {
        id: "ip:ti/am335x-timer",
        summary: "TI AM335x DMTimer",
        outputs: &[],
        spawn: spawn_am335x_timer,
    },
    ModelEntry {
        id: "ip:microchip/pic32-timer",
        summary: "Microchip PIC32 timer",
        outputs: &[],
        spawn: spawn_pic32_timer,
    },
    ModelEntry {
        id: "ip:silabs/efm32-timer",
        summary: "Silicon Labs EFM32 timer",
        outputs: &[],
        spawn: spawn_efm32_timer,
    },
    ModelEntry {
        id: "ip:intel/i8259-pit",
        summary: "Intel 8259 PIC chained timer",
        outputs: &[],
        spawn: spawn_i8259_pit,
    },
    ModelEntry {
        id: "ip:renesas/rza1-timer",
        summary: "Renesas RZ/A1 timer",
        outputs: &[],
        spawn: spawn_rza1_timer,
    },
];

/// Default spawn for i8253-pit: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i8253_pit(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I8253Pit::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i8254-pit: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i8254_pit(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I8254Pit::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sp805: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sp805(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Sp805::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for tim-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_tim_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = TimF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for tim-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_tim_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = TimF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for tim-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_tim_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = TimH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for tim-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_tim_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = TimL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_timer(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxTimer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc17xx-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_timer(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxTimer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for kinetis-pit: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_kinetis_pit(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = KinetisPit::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for kinetis-ftm: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_kinetis_ftm(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = KinetisFtm::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for kinetis-lptmr: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_kinetis_lptmr(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = KinetisLptmr::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Timer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-pwm: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_pwm(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Pwm::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sunxi-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiTimer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rk-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkTimer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mtk-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkTimer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am335x-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XTimer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pic32-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32Timer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for efm32-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efm32_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efm32Timer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for i8259-pit: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i8259_pit(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I8259Pit::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rza1-timer: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rza1_timer(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rza1Timer::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
