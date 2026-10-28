//! Embedded controller, super-I/O and BMC parts behind the LPC/eSPI
//! fabrics (ITE, MEC, ENE, SMSC, ASPEED) — the board-management chips,
//! distinct from the CPU-addressed platform glue in `crate::ip::platform`.
pub mod aspeed_ast2600_espi;
pub mod catalog;
pub mod ene_kb3926;
pub mod intel_spt_lpss_spi;
pub mod ite_it8528;
pub mod ite_it8528e;
pub mod ite_it8728f;
pub mod mec1428;
pub mod mec1633;
pub mod smsc_mec1308;
