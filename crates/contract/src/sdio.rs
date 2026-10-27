//! `sdio` contract: SD I/O card bus (functions 0-7, CMD52/CMD53).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SdioError {
    NoCard,
    CrcFail,
    Timeout,
    BadFunction,
    UnsupportedVoltage,
}

/// A card command (CMD index + 32-bit argument).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SdioCommand {
    pub cmd: u8,
    pub arg: u32,
}

pub trait SdioHost {
    fn send_cmd(&mut self, ctx: &mut dyn Ctx, command: &SdioCommand) -> Result<Vec<u8>, SdioError>;
    /// CMD52: direct byte read/write on one function's address space.
    fn io_rw_direct(
        &mut self,
        ctx: &mut dyn Ctx,
        function: u8,
        address: u32,
        write: bool,
        data: u8,
    ) -> Result<u8, SdioError>;
}

pub trait SdioCard {
    fn on_cmd(&mut self, ctx: &mut dyn Ctx, command: &SdioCommand) -> Result<Vec<u8>, SdioError>;
    fn io_rw_direct(
        &mut self,
        ctx: &mut dyn Ctx,
        function: u8,
        address: u32,
        write: bool,
        data: u8,
    ) -> Result<u8, SdioError>;
}
