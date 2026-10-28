//! CPU-addressed platform glue (ADR 0031 §4): PCH service blocks,
//! PC speaker, the QEMU test device, PrimeCell ID registers. External
//! EC/SIO/BMC chips live in `crate::part::ec`.
pub mod ast_lpc;
pub mod catalog;
pub mod csme;
pub mod ish;
pub mod npcm_lpc;
pub mod p2sb;
pub mod pc_testdev;
pub mod pcspk;
pub mod pmc;
pub mod pmt;
