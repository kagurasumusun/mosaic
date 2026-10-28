//! DVI monitor sink core (single/dual-link TMDS + DDC EDID) — the shared
//! functional core behind every `part:*/...` DVI monitor.
//!
//! One family, one core (ADR 0026 step 2 pattern): a monitor is an EDID, a
//! hot-plug detect (HPD) line the source must observe, and the link width
//! (1 = single link up to 165 MHz, 2 = dual link) that gates whether a frame
//! fits the configured bandwidth.

use mosaic_contract::dvi::{DviError, DviFrame, DviSink};
use mosaic_contract::model::Ctx;

/// A minimal EDID 1.3 base block with header and checksum.
fn edid_base() -> [u8; 128] {
    let mut edid = [0u8; 128];
    edid[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    let sum: u8 = edid[..127].iter().fold(0u8, |acc, b| acc.wrapping_sub(*b));
    edid[127] = sum;
    edid
}

/// One DVI monitor.
pub struct DviMonitorCore {
    lanes: u32,
    edid: Vec<u8>,
    hpd: bool,
    frames: u64,
}

impl Default for DviMonitorCore {
    fn default() -> Self {
        Self::new()
    }
}

impl DviMonitorCore {
    /// A single-link (165 MHz) DVI-D monitor with HPD asserted.
    pub fn new() -> Self {
        Self {
            lanes: 1,
            edid: edid_base().to_vec(),
            hpd: true,
            frames: 0,
        }
    }

    /// Link width: 1 (single) or 2 (dual).
    pub fn with_lanes(mut self, lanes: u32) -> Self {
        self.lanes = lanes.max(1);
        self
    }

    /// Replace the EDID base block (128 bytes) if the length matches.
    pub fn with_edid(mut self, edid: Vec<u8>) -> Self {
        if edid.len() == 128 {
            self.edid = edid;
        }
        self
    }

    /// Frames delivered since attach.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Configured link width.
    pub fn lanes(&self) -> u32 {
        self.lanes
    }

    /// Assert or deassert the hot-plug detect line (cable pull = deassert).
    pub fn set_hpd(&mut self, hpd: bool) {
        self.hpd = hpd;
    }
}

impl DviSink for DviMonitorCore {
    fn on_video(&mut self, _ctx: &mut dyn Ctx, frame: &DviFrame) -> Result<(), DviError> {
        if !self.hpd {
            return Err(DviError::HpdDeasserted);
        }
        // A single link cannot carry a frame wider than the 165 MHz TMDS
        // ceiling; dual-link doubles it.
        let ceiling = 165_000_000u64 * self.lanes as u64;
        let needed = frame.width as u64 * frame.height as u64 * 60;
        if needed > ceiling {
            return Err(DviError::BadFrame);
        }
        self.frames += 1;
        Ok(())
    }

    fn edid(&self) -> &[u8] {
        &self.edid
    }

    fn hpd(&self) -> bool {
        self.hpd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopCtx;
    impl Ctx for NoopCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _: mosaic_exec::VTime,
            _: u64,
        ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(
            &mut self,
            _: mosaic_exec::SignalId,
            _: bool,
        ) -> Result<(), mosaic_contract::model::ContextError> {
            Ok(())
        }
    }

    fn frame(w: u32, h: u32) -> DviFrame {
        DviFrame {
            width: w,
            height: h,
            pixels: vec![0; 8],
        }
    }

    #[test]
    fn uxga_fits_single_link() {
        let mut monitor = DviMonitorCore::new();
        let mut ctx = NoopCtx;
        assert!(monitor.on_video(&mut ctx, &frame(1600, 1200)).is_ok());
        assert_eq!(monitor.frames(), 1);
    }

    #[test]
    fn hpd_low_rejects_video() {
        let mut monitor = DviMonitorCore::new();
        monitor.set_hpd(false);
        let mut ctx = NoopCtx;
        assert_eq!(
            monitor.on_video(&mut ctx, &frame(800, 600)),
            Err(DviError::HpdDeasserted)
        );
    }

    #[test]
    fn oversized_frame_needs_dual_link() {
        let mut single = DviMonitorCore::new().with_lanes(1);
        let mut ctx = NoopCtx;
        assert_eq!(
            single.on_video(&mut ctx, &frame(2560, 1600)),
            Err(DviError::BadFrame)
        );
        let mut dual = DviMonitorCore::new().with_lanes(2);
        assert!(dual.on_video(&mut ctx, &frame(2560, 1600)).is_ok());
        assert_eq!(dual.lanes(), 2);
    }

    #[test]
    fn edid_checksum_is_valid() {
        let monitor = DviMonitorCore::new();
        let sum: u32 = monitor.edid().iter().map(|b| *b as u32).sum();
        assert_eq!(sum % 256, 0);
    }
}
