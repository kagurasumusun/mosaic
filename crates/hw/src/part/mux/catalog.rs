//! Bus switches, multiplexers and bridges (TCA954x, PCA954x, the
//! 1-Wire-to-I2C bridge): parts that fan one fabric out into several.
//!
//! The TCA9548A is the two-sided fabric part (ADR 0032): its upstream
//! `i2c` face is a target on the master's bus, and each downstream
//! `down0..down7` face is an initiator segment other parts attach to.

use std::sync::{Arc, Mutex};

use mosaic_assemble::{ConfigValue, InstantiateCtx, MaterializeError};
use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cTarget};
use mosaic_contract::model::Ctx;
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::part::mux::tca9548a::{Tca9548a, CHANNELS};

/// The switch behind its upstream target face: the master's bus owns this
/// wrapper; the segment buses live in the shared core.
struct Tca9548aI2c(Arc<Mutex<Tca9548a>>);

impl I2cTarget for Tca9548aI2c {
    fn matches(&self, addr: &I2cAddr) -> bool {
        self.0.lock().unwrap().matches(addr)
    }
    fn target_write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        bytes: &[u8],
    ) -> Result<Ack, I2cError> {
        self.0.lock().unwrap().target_write(ctx, addr, bytes)
    }
    fn target_read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: &I2cAddr,
        buf: &mut [u8],
    ) -> Result<Ack, I2cError> {
        self.0.lock().unwrap().target_read(ctx, addr, buf)
    }
}

/// TCA9548A: the `i2c` face answers at the wired control address, and the
/// eight `down0..down7` faces are the segments downstream parts ride.
fn spawn_tca9548a(ctx: &InstantiateCtx, _machine: &mut Machine) -> Result<(), MaterializeError> {
    let address = ctx
        .face_params("i2c")
        .and_then(|params| params.get("address"))
        .and_then(ConfigValue::as_integer)
        .unwrap_or(0x70) as u8;
    let switch = Arc::new(Mutex::new(Tca9548a::new(address).map_err(|error| {
        MaterializeError::Bind {
            node: ctx.graph_id.to_owned(),
            detail: format!("tca9548a strap address {address:#x}: {error:?}"),
        }
    })?));
    for channel in 0..CHANNELS {
        ctx.offer_i2c_segment(
            &format!("down{channel}"),
            switch.lock().unwrap().channel_bus(channel),
        );
    }
    ctx.expose_i2c_target("i2c", Box::new(Tca9548aI2c(Arc::clone(&switch))));
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "part:ti/tca9548a",
    summary: "TCA9548A 8-channel I2C switch (upstream i2c face + 8 downstream segment faces)",
    outputs: &[],
    spawn: spawn_tca9548a,
}];
