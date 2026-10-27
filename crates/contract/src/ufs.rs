//! `ufs` contract: Universal Flash Storage (UPIU exchange).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UfsError {
    NoDevice,
    BadUpiu,
    ScsiCheckCondition,
    Timeout,
}

/// One UFS Protocol Information Unit (task type + payload).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Upiu {
    pub kind: u8,
    pub task_tag: u8,
    pub data: Vec<u8>,
}

pub trait UfsHost {
    fn submit_upiu(&mut self, ctx: &mut dyn Ctx, upiu: &Upiu) -> Result<Upiu, UfsError>;
}

pub trait UfsDevice {
    fn on_upiu(&mut self, ctx: &mut dyn Ctx, upiu: &Upiu) -> Result<Upiu, UfsError>;
}
