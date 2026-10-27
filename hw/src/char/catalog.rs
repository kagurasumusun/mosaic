//! Serial character devices: catalog entries (ADR 0025). A model's default
//! instantiation is the model's own knowledge — parameters from the manifest,
//! IRQ output from the declared edge, console stream from the machine
//! harness's host attachment.

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_exec::SignalId;
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::char::ns16550a::Ns16550;
use crate::char::pl011::{Pl011, Pl011Params};
use crate::char::usart_f1::{StUsartF1, StUsartF1Params};

/// STM32F1 USART: `combined` IRQ output, `clock` input, optional console.
fn spawn_usart_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let mut usart = StUsartF1::new(
        StUsartF1Params::new(irq),
        ctx.host_uart().cloned().map(|endpoint| {
            let stream: Box<dyn mosaic_contract::stream::ByteStream> = Box::new(endpoint);
            stream
        }),
    );
    if let Some(wire) = ctx.incoming.iter().find(|wire| wire.port == "clock") {
        usart.bind_clock(wire.source, wire.signal);
    }
    machine.register_device(ctx.node, Box::new(usart))?;
    Ok(())
}

/// ARM PrimeCell UART (PL011): `combined` IRQ output, optional console.
fn spawn_pl011(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let uart = Pl011::new(
        Pl011Params::new(irq),
        ctx.host_uart().cloned().map(|endpoint| {
            let stream: Box<dyn mosaic_contract::stream::ByteStream> = Box::new(endpoint);
            stream
        }),
    );
    machine.register_device(ctx.node, Box::new(uart))?;
    Ok(())
}

/// NS16550A UART: `irq` output, optional console. Parameters: none (the
/// 16550 carries its configuration in registers, not wiring).
fn spawn_ns16550(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mut uart = Ns16550::new(irq);
    if let Some(endpoint) = ctx.host_uart() {
        uart.connect_host(endpoint.clone());
    }
    machine.register_device(ctx.node, Box::new(uart))?;
    Ok(())
}

/// The unused-import marker for `SignalId` (kept for the output tables the
/// future models in this directory publish).
#[allow(unused_imports)]
use SignalId as _SignalId;

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/pl011",
        summary: "ARM PrimeCell UART (PL011) functional register model",
        outputs: &[],
        spawn: spawn_pl011,
    },
    ModelEntry {
        id: "ip:ns/ns16550a",
        summary: "NS16550A UART: FIFOs, modem lines, full register set (PC COM ports)",
        outputs: &[],
        spawn: spawn_ns16550,
    },
    ModelEntry {
        id: "ip:st/usart-f1",
        summary: "STM32F1 USART (RM0008) with clock gate and combined IRQ",
        outputs: &[],
        spawn: spawn_usart_f1,
    },
];
