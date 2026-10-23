#![no_std]
#![forbid(unsafe_code)]
//! Device-local offsets only; wiring and address translation belong to runtime.

use mosaic_contract_model::Ctx;
use mosaic_core_types::{AccessSize, Attr, BusFault};

pub trait MmioTarget: Send {
    /// Debug reads (Attr::kind == Debug) must not consume data or clear status.
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault>;

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        attr: &Attr,
    ) -> Result<(), BusFault>;
}
