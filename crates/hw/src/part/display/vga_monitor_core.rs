//! VGA monitor sink core (CRT/LCD, analog RGB + DDC EDID) — the shared
//! functional core behind every `part:*/...` VGA monitor.
//!
//! One family, one core (ADR 0026 step 2 pattern): a monitor is a 128-byte
//! EDID (DDC2B), the accepted mode bookkeeping, and a frame counter. The
//! monitor rejects modes whose pixel clock exceeds its video bandwidth — the
//! classic "out of range" OSD state — which is the only analog-monitor
//! behavior a source can observe over the fabric.

use mosaic_contract::model::Ctx;
use mosaic_contract::vga::{VgaError, VgaFrame, VgaMode, VgaSink};

/// A minimal EDID 1.3 base block with the header, extension count and
/// checksum filled in. Establish timings and descriptors are per-monitor
/// configuration.
fn edid_base() -> [u8; 128] {
    let mut edid = [0u8; 128];
    edid[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    edid[126] = 0x00; // extension count
    let sum: u8 = edid[..127].iter().fold(0u8, |acc, b| acc.wrapping_sub(*b));
    edid[127] = sum;
    edid
}

/// One VGA monitor.
pub struct VgaMonitorCore {
    max_pixel_clock_khz: u32,
    edid: Vec<u8>,
    mode: Option<VgaMode>,
    frames: u64,
    out_of_range: bool,
}

impl Default for VgaMonitorCore {
    fn default() -> Self {
        Self::new()
    }
}

impl VgaMonitorCore {
    /// A monitor with a 180 MHz video bandwidth (17-19" class).
    pub fn new() -> Self {
        Self {
            max_pixel_clock_khz: 180_000,
            edid: edid_base().to_vec(),
            mode: None,
            frames: 0,
            out_of_range: false,
        }
    }

    /// Video bandwidth (max pixel clock) in kHz.
    pub fn with_max_pixel_clock_khz(mut self, khz: u32) -> Self {
        self.max_pixel_clock_khz = khz;
        self
    }

    /// Replace the EDID with a monitor-specific 128-byte base block.
    pub fn with_edid(mut self, edid: Vec<u8>) -> Self {
        if edid.len() == 128 {
            self.edid = edid;
        }
        self
    }

    /// The accepted mode, if any.
    pub fn mode(&self) -> Option<VgaMode> {
        self.mode
    }

    /// Frames delivered since attach.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// True when the last mode exceeded the video bandwidth ("out of range").
    pub fn out_of_range(&self) -> bool {
        self.out_of_range
    }
}

impl VgaSink for VgaMonitorCore {
    fn on_mode(&mut self, _ctx: &mut dyn Ctx, mode: &VgaMode) -> Result<(), VgaError> {
        if mode.pixel_clock_khz > self.max_pixel_clock_khz {
            self.out_of_range = true;
            self.mode = None;
            return Err(VgaError::BadMode);
        }
        self.out_of_range = false;
        self.mode = Some(*mode);
        Ok(())
    }

    fn on_frame(&mut self, _ctx: &mut dyn Ctx, frame: &VgaFrame) -> Result<(), VgaError> {
        if self.mode.is_none() {
            // A monitor without an accepted mode shows no signal: the frame
            // is dropped, not an error, mirroring a real CRT's blank screen.
            let _ = frame;
            return Ok(());
        }
        self.frames += 1;
        Ok(())
    }

    fn edid(&self) -> &[u8] {
        &self.edid
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

    fn mode(clock_khz: u32) -> VgaMode {
        VgaMode {
            width: 1600,
            height: 1200,
            hz: 60,
            pixel_clock_khz: clock_khz,
        }
    }

    #[test]
    fn mode_within_bandwidth_is_accepted() {
        let mut monitor = VgaMonitorCore::new().with_max_pixel_clock_khz(300_000);
        let mut ctx = NoopCtx;
        assert!(monitor.on_mode(&mut ctx, &mode(280_000)).is_ok());
        assert_eq!(monitor.mode(), Some(mode(280_000)));
        assert!(!monitor.out_of_range());
    }

    #[test]
    fn mode_over_bandwidth_is_out_of_range() {
        let mut monitor = VgaMonitorCore::new().with_max_pixel_clock_khz(250_000);
        let mut ctx = NoopCtx;
        assert_eq!(
            monitor.on_mode(&mut ctx, &mode(300_000)),
            Err(VgaError::BadMode)
        );
        assert!(monitor.out_of_range());
        assert_eq!(monitor.mode(), None);
    }

    #[test]
    fn frames_count_only_after_an_accepted_mode() {
        let mut monitor = VgaMonitorCore::new();
        let mut ctx = NoopCtx;
        let frame = VgaFrame {
            width: 640,
            height: 480,
            pixels: vec![0; 8],
        };
        assert!(monitor.on_frame(&mut ctx, &frame).is_ok());
        assert_eq!(monitor.frames(), 0);
        assert!(monitor.on_mode(&mut ctx, &mode(100_000)).is_ok());
        assert!(monitor.on_frame(&mut ctx, &frame).is_ok());
        assert!(monitor.on_frame(&mut ctx, &frame).is_ok());
        assert_eq!(monitor.frames(), 2);
    }

    #[test]
    fn edid_has_a_valid_checksum() {
        let monitor = VgaMonitorCore::new();
        let edid = monitor.edid();
        assert_eq!(
            &edid[0..8],
            &[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]
        );
        let sum: u32 = edid.iter().map(|b| *b as u32).sum();
        assert_eq!(sum % 256, 0);
    }

    #[test]
    fn custom_edid_wins_only_at_full_length() {
        let monitor = VgaMonitorCore::new().with_edid(vec![0xAA; 8]);
        assert_eq!(monitor.edid()[0], 0x00, "short EDID is ignored");
        let full = [0x51u8; 128];
        let monitor = VgaMonitorCore::new().with_edid(full.to_vec());
        assert_eq!(monitor.edid()[0], 0x51);
    }
}
