//! GPIO controllers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::gpio::gpio_f1::{GpioF1, GpioF1Params};
use crate::gpio::pl061::{Pl061, Pl061Params};

/// STM32F1 GPIO port: `clock` input from the RCC gate.
fn spawn_gpio_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let mut gpio = GpioF1::new(GpioF1Params);
    if let Some(wire) = ctx.incoming.iter().find(|wire| wire.port == "clock") {
        gpio.bind_clock(wire.source, wire.signal);
    }
    machine.register_device(ctx.node, Box::new(gpio))?;
    Ok(())
}

/// ARM PrimeCell GPIO (PL061): `combined` IRQ output.
fn spawn_pl061(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let gpio = Pl061::new(Pl061Params::new(irq));
    machine.register_device(ctx.node, Box::new(gpio))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:st/gpio-f1",
        summary: "STM32F1 GPIO port (RM0008) with clock gate and pin signals",
        outputs: &[],
        spawn: spawn_gpio_f1,
    },
    ModelEntry {
        id: "ip:arm/pl061",
        summary: "ARM PrimeCell GPIO (PL061) functional register model",
        outputs: &[],
        spawn: spawn_pl061,
    },
];
