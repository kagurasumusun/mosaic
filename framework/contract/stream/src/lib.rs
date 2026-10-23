#![no_std]
#![forbid(unsafe_code)]
//! Byte and frame stream contracts.
//!
//! UART endpoints exchange byte streams while Ethernet-style links exchange
//! length-prefixed frames (構成設計書 §11). Implementations are responsible
//! for fixed, finite buffering: capacity errors must surface as
//! [`StreamError::Full`], never as a panic.

use mosaic_contract_model::Ctx;

/// Back-pressure and connectivity errors shared by both stream shapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StreamError {
    /// The destination buffer has no free slot.
    Full,
    /// The peer endpoint was removed or never attached.
    NotConnected,
    /// The link was deliberately shut down.
    Closed,
}

/// Unframed byte stream, the transport shape of UART lines and pipes.
pub trait ByteStream: Send {
    /// Push as many bytes as fit; returns the number accepted.
    fn write_bytes(
        &mut self,
        ctx: &mut dyn Ctx,
        bytes: &[u8],
    ) -> Result<usize, StreamError>;

    /// Pull up to `buf.len()` buffered bytes; zero means no data is waiting.
    fn read_bytes(
        &mut self,
        ctx: &mut dyn Ctx,
        buf: &mut [u8],
    ) -> Result<usize, StreamError>;
}

/// Datagram-style link for network frames.
pub trait FrameStream: Send {
    /// Maximum payload accepted by one frame.
    fn mtu(&self) -> usize;

    /// Deliver one complete frame. Oversized input is an error, not a split.
    fn send_frame(
        &mut self,
        ctx: &mut dyn Ctx,
        frame: &[u8],
    ) -> Result<(), StreamError>;

    /// Copy one received frame into `buf`; returns its length.
    fn recv_frame(
        &mut self,
        ctx: &mut dyn Ctx,
        buf: &mut [u8],
    ) -> Result<usize, StreamError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_variants_remain_comparable_for_backpressure_handling() {
        assert_eq!(StreamError::Full, StreamError::Full);
        assert_ne!(StreamError::Full, StreamError::Closed);
    }
}
