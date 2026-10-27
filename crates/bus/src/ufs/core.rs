//! `ufs` bus entity (ADR 0029): Universal Flash Storage (UPIU exchange).

use mosaic_contract::model::Ctx;
use mosaic_contract::ufs::{UfsDevice, UfsError, UfsHost, Upiu};

pub struct UfsBus {
    device: Option<Box<dyn UfsDevice>>,
}

impl Default for UfsBus {
    fn default() -> Self {
        Self::new()
    }
}

impl UfsBus {
    pub fn new() -> Self {
        Self { device: None }
    }
    pub fn attach_device(&mut self, d: Box<dyn UfsDevice>) {
        self.device = Some(d);
    }
}

impl UfsHost for UfsBus {
    fn submit_upiu(&mut self, ctx: &mut dyn Ctx, upiu: &Upiu) -> Result<Upiu, UfsError> {
        match self.device.as_deref_mut() {
            Some(d) => {
                if upiu.task_tag == 0 {
                    return Err(UfsError::BadUpiu);
                }
                d.on_upiu(ctx, upiu)
            }
            None => Err(UfsError::NoDevice),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::ufs::UfsDevice;

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

    struct FlashDevice;
    impl UfsDevice for FlashDevice {
        fn on_upiu(&mut self, _ctx: &mut dyn Ctx, upiu: &Upiu) -> Result<Upiu, UfsError> {
            Ok(Upiu {
                kind: upiu.kind | 0x80,
                task_tag: upiu.task_tag,
                data: upiu.data.clone(),
            })
        }
    }

    #[test]
    fn upiu_round_trips_the_device() {
        let mut bus = UfsBus::new();
        bus.attach_device(Box::new(FlashDevice));
        let mut ctx = NoopCtx;
        let upiu = Upiu {
            kind: 0x16,
            task_tag: 1,
            data: vec![0; 20],
        };
        let response = bus.submit_upiu(&mut ctx, &upiu).unwrap();
        assert_eq!(response.kind, 0x96);
    }

    #[test]
    fn no_device_yields_error() {
        let mut bus = UfsBus::new();
        let mut ctx = NoopCtx;
        let upiu = Upiu {
            task_tag: 1,
            ..Upiu::default()
        };
        assert_eq!(bus.submit_upiu(&mut ctx, &upiu), Err(UfsError::NoDevice));
    }

    #[test]
    fn task_tag_zero_is_a_bad_upiu() {
        let mut bus = UfsBus::new();
        bus.attach_device(Box::new(FlashDevice));
        let mut ctx = NoopCtx;
        assert_eq!(
            bus.submit_upiu(&mut ctx, &Upiu::default()),
            Err(UfsError::BadUpiu)
        );
    }
}
