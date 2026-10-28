//! Embedded controller, super-I/O and BMC parts behind the LPC/eSPI
//! fabrics (ITE, MEC, ENE, SMSC, ASPEED) — the board-management chips,
//! distinct from the CPU-addressed platform glue in `crate::ip::platform`.

use crate::catalog::ModelEntry;

pub const MODELS: &[ModelEntry] = &[];
