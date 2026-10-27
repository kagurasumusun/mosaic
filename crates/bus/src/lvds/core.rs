//! `lvds` bus entity (ADR 0029): LVDS LCD panel link.

use mosaic_contract::lvds::{LvdsError, LvdsFrame, LvdsSink, LvdsSource};
use mosaic_contract::model::Ctx;

pub struct LvdsBus {
    panel: Option<Box<dyn LvdsSink>>,
}

impl Default for LvdsBus {
    fn default() -> Self {
        Self::new()
    }
}

impl LvdsBus {
    pub fn new() -> Self {
        Self { panel: None }
    }
    pub fn attach_panel(&mut self, p: Box<dyn LvdsSink>) {
        self.panel = Some(p);
    }
}

impl LvdsSource for LvdsBus {
    fn set_backlight(&mut self, ctx: &mut dyn Ctx, level: u8) -> Result<(), LvdsError> {
        match self.panel.as_deref_mut() {
            Some(p) => p.on_backlight(ctx, level),
            None => Err(LvdsError::NoPanel),
        }
    }
    fn push_frame(&mut self, ctx: &mut dyn Ctx, frame: &LvdsFrame) -> Result<(), LvdsError> {
        match self.panel.as_deref_mut() {
            Some(p) => {
                if p.backlight() == 0 {
                    return Err(LvdsError::BacklightOff);
                }
                p.on_frame(ctx, frame)
            }
            None => Err(LvdsError::NoPanel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::lvds::{LvdsFrame, LvdsSink};

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

    struct Panel {
        backlight: u8,
        frames: u32,
    }
    impl LvdsSink for Panel {
        fn on_backlight(&mut self, _ctx: &mut dyn Ctx, level: u8) -> Result<(), LvdsError> {
            self.backlight = level;
            Ok(())
        }
        fn on_frame(&mut self, _ctx: &mut dyn Ctx, _f: &LvdsFrame) -> Result<(), LvdsError> {
            self.frames += 1;
            Ok(())
        }
        fn backlight(&self) -> u8 {
            self.backlight
        }
    }

    #[test]
    fn frame_reaches_lit_panel() {
        let mut bus = LvdsBus::new();
        bus.attach_panel(Box::new(Panel {
            backlight: 128,
            frames: 0,
        }));
        let mut ctx = NoopCtx;
        let frame = LvdsFrame {
            width: 1024,
            height: 768,
            pixels: vec![0; 16],
        };
        assert!(bus.push_frame(&mut ctx, &frame).is_ok());
    }

    #[test]
    fn no_panel_yields_error() {
        let mut bus = LvdsBus::new();
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.push_frame(&mut ctx, &LvdsFrame::default()),
            Err(LvdsError::NoPanel)
        );
    }

    #[test]
    fn dark_panel_rejects_frames() {
        let mut bus = LvdsBus::new();
        bus.attach_panel(Box::new(Panel {
            backlight: 0,
            frames: 0,
        }));
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.push_frame(&mut ctx, &LvdsFrame::default()),
            Err(LvdsError::BacklightOff)
        );
    }
}
