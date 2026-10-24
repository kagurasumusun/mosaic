#![no_std]
#![forbid(unsafe_code)]
//! Digital GPIO pin contract.
//!
//! A model drives an output pin through the [`GpioPort`] it received at
//! construction and observes input levels through [`GpioSink`]. Electrical
//! resolution (pull resistors, open drain, tristate and collision detection)
//! belongs to the net entity in `mosaic-interconnect-gpio-net`, never to the
//! model. Design: 構成設計書 §11, §13.3.

use mosaic_contract_model::Ctx;

/// Resolved digital voltage level presented to a pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PinLevel {
    Low,
    High,
}

impl PinLevel {
    pub const fn high(self) -> bool {
        matches!(self, Self::High)
    }
}

/// Signal direction as configured by the owning model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PinDirection {
    Input,
    Output,
}

/// Pull resistor attached to the pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Pull {
    None,
    Up,
    Down,
}

/// Output drive topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DriveMode {
    PushPull,
    OpenDrain,
}

/// Full pin configuration. The constructor keeps the type usable even though
/// the struct is `#[non_exhaustive]` for forward compatibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PinConfig {
    pub direction: PinDirection,
    pub pull: Pull,
    pub drive: DriveMode,
    pub initial: PinLevel,
}

impl PinConfig {
    pub const fn new(
        direction: PinDirection,
        pull: Pull,
        drive: DriveMode,
        initial: PinLevel,
    ) -> Self {
        Self {
            direction,
            pull,
            drive,
            initial,
        }
    }

    /// Conventional push-pull output starting at the given level.
    pub const fn output(initial: PinLevel) -> Self {
        Self::new(
            PinDirection::Output,
            Pull::None,
            DriveMode::PushPull,
            initial,
        )
    }

    /// Conventional high-impedance input with the given pull.
    pub const fn input(pull: Pull) -> Self {
        Self::new(
            PinDirection::Input,
            pull,
            DriveMode::PushPull,
            PinLevel::Low,
        )
    }
}

/// GPIO port failures. Models must keep running on every variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GpioError {
    /// The composition root never attached this port to a net.
    Disconnected,
    /// The net rejected the requested configuration.
    Unsupported,
    /// A sink or port is already attached to this node/function.
    AlreadyConnected,
    /// A downstream port is already executing a callback through this mux.
    Reentrant,
}

/// Output-side port held by a model and implemented by the net adapter.
///
/// Calls happen inside an existing model callback, so the same scoped
/// [`Ctx`] is forwarded for any deferred effect the adapter may schedule.
pub trait GpioPort: Send {
    /// Drive an output pin. Open-drain ports map [`PinLevel::High`] to high-Z.
    fn drive(&mut self, ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError>;

    /// Release the pin to high impedance and notify any connected sinks.
    /// A direction change to input must use this instead of merely changing
    /// configuration, because configuration alone cannot carry a runtime Ctx.
    fn release(&mut self, ctx: &mut dyn Ctx) -> Result<(), GpioError>;

    /// Apply a new pin configuration. This does not itself drive or release
    /// the electrical line; call `drive` or `release` for that transition.
    fn configure(&mut self, config: PinConfig) -> Result<(), GpioError>;

    /// Last resolved level observable on the pin, for input read-back.
    fn current_level(&self) -> PinLevel;
}

/// Input-side callback implemented by models that observe pin levels.
pub trait GpioSink: Send {
    fn level_changed(&mut self, ctx: &mut dyn Ctx, level: PinLevel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_constructors_cover_the_two_default_roles() {
        let output = PinConfig::output(PinLevel::High);
        assert_eq!(output.direction, PinDirection::Output);
        assert_eq!(output.drive, DriveMode::PushPull);
        assert_eq!(output.initial, PinLevel::High);
        let input = PinConfig::input(Pull::Up);
        assert_eq!(input.direction, PinDirection::Input);
        assert_eq!(input.pull, Pull::Up);
    }

    #[test]
    fn level_reports_its_boolean_state() {
        assert!(PinLevel::High.high());
        assert!(!PinLevel::Low.high());
    }
}
