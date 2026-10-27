//! Timers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::timer::pit_8254::Pit8254;
use crate::timer::sp804::{Sp804, Sp804Params};

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
];
