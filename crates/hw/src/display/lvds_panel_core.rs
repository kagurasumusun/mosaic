//! LVDS panel sink core (notebook/open-frame panels) — the shared functional
//! core behind every `part:*/...` LVDS panel.
//!
//! One family, one core (ADR 0026 step 2 pattern): a panel is a backlight
//! level (0 = off, dark frames are rejected), the native timing it accepts,
//! and a frame counter. The panel accepts only frames matching its native
//! panel resolution — scaling is the bridge's job, not the panel's.

use mosaic_contract::lvds::{LvdsError, LvdsFrame, LvdsSink};
use mosaic_contract::model::Ctx;

/// One LVDS panel.
pub struct LvdsPanelCore {
    width: u32,
    height: u32,
    lanes: u32,
    backlight: u8,
    frames: u64,
}

impl Default for LvdsPanelCore {
    fn default() -> Self {
        // The most common panel in the wild: 15.4" WXGA, 4-lane.
        Self::new()
    }
}

impl LvdsPanelCore {
    /// A 1280x800 4-lane panel with the backlight off.
    pub fn new() -> Self {
        Self {
            width: 1280,
            height: 800,
            lanes: 4,
            backlight: 0,
            frames: 0,
        }
    }

    /// Native panel resolution.
    pub fn with_resolution(mut self, width: u32, height: u32) -> Self {
        if width > 0 && height > 0 {
            self.width = width;
            self.height = height;
        }
        self
    }

    /// LVDS data lane count (4 or 8).
    pub fn with_lanes(mut self, lanes: u32) -> Self {
        self.lanes = lanes.max(1);
        self
    }

    /// Configured data lane count.
    pub fn lanes(&self) -> u32 {
        self.lanes
    }

    /// Frames displayed since attach.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Current backlight level (0-255).
    pub fn backlight(&self) -> u8 {
        self.backlight
    }
}

impl LvdsSink for LvdsPanelCore {
    fn on_backlight(&mut self, _ctx: &mut dyn Ctx, level: u8) -> Result<(), LvdsError> {
        self.backlight = level;
        Ok(())
    }

    fn on_frame(&mut self, _ctx: &mut dyn Ctx, frame: &LvdsFrame) -> Result<(), LvdsError> {
        if self.backlight == 0 {
            return Err(LvdsError::BacklightOff);
        }
        if frame.width != self.width || frame.height != self.height {
            // A panel has no scaler: only its native timing lights up.
            return Err(LvdsError::BadTiming);
        }
        self.frames += 1;
        Ok(())
    }

    fn backlight(&self) -> u8 {
        self.backlight
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

    fn frame(w: u32, h: u32) -> LvdsFrame {
        LvdsFrame {
            width: w,
            height: h,
            pixels: vec![0; 8],
        }
    }

    #[test]
    fn native_timing_lights_the_panel() {
        let mut panel = LvdsPanelCore::new();
        let mut ctx = NoopCtx;
        assert!(panel.on_backlight(&mut ctx, 200).is_ok());
        assert!(panel.on_frame(&mut ctx, &frame(1280, 800)).is_ok());
        assert_eq!(panel.frames(), 1);
        assert_eq!(panel.backlight(), 200);
    }

    #[test]
    fn dark_panel_rejects_frames() {
        let mut panel = LvdsPanelCore::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            panel.on_frame(&mut ctx, &frame(1280, 800)),
            Err(LvdsError::BacklightOff)
        );
    }

    #[test]
    fn non_native_timing_is_rejected() {
        let mut panel = LvdsPanelCore::new().with_resolution(1024, 768);
        let mut ctx = NoopCtx;
        assert!(panel.on_backlight(&mut ctx, 255).is_ok());
        assert_eq!(
            panel.on_frame(&mut ctx, &frame(1280, 800)),
            Err(LvdsError::BadTiming)
        );
        assert!(panel.on_frame(&mut ctx, &frame(1024, 768)).is_ok());
    }

    #[test]
    fn lanes_and_resolution_builders_are_clamped() {
        let panel = LvdsPanelCore::new().with_lanes(8).with_resolution(0, 0);
        assert_eq!(panel.lanes(), 8);
        // A zero resolution cannot displace the native default.
        let mut panel = LvdsPanelCore::new().with_resolution(0, 0);
        let mut ctx = NoopCtx;
        panel.on_backlight(&mut ctx, 1).unwrap();
        assert!(panel.on_frame(&mut ctx, &frame(1280, 800)).is_ok());
    }
}
