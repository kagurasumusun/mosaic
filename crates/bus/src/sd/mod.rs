//! SD/eMMC interconnect: the command port between a host controller and a
//! card (the wire `CMD`/`DAT0..7` lines carry, at transaction level).
//!
//! Bus membership is board wiring (ADR 0025): a board creates the card, the
//! host controller asks for it through its host attachment, and the two
//! speak through [`SdCardHandle`] — a shareable handle exactly like the I2C
//! and USB fabrics in this crate.

pub mod card;

pub use card::{card_handle, SdCardHandle, SdCardPort, SdError, SdResponse, SdResponseType};
