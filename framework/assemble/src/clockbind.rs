//! Resolve a core's instruction period from the runtime clock tree.
//!
//! Catalog timers still take a period parameter: they cannot depend on the
//! runtime. Composition calls [`instruction_period`] and passes the result to
//! SP804 or to [`crate::MulticoreDriver`] when the unicorn feature is enabled.

use core::num::NonZeroU64;

use mosaic_core_runtime::{ClockError, ClockId, ClockTree};

/// Picoseconds of one tick of `clock`. A gated or sub-picosecond clock is an
/// error rather than a zero period that would stall virtual time.
pub fn instruction_period(tree: &ClockTree, clock: ClockId) -> Result<NonZeroU64, ClockError> {
    tree.period_ps(clock)
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use mosaic_core_runtime::ClockTree;

    use super::*;

    #[test]
    fn a_divided_source_becomes_the_instruction_period() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(NonZeroU64::new(24_000_000).unwrap());
        let uart = tree
            .add_divider(source, NonZeroU64::new(2).unwrap())
            .unwrap();
        assert_eq!(
            instruction_period(&tree, uart).unwrap().get(),
            1_000_000_000_000 / 12_000_000
        );
    }

    #[test]
    fn a_closed_gate_cannot_drive_a_core() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(NonZeroU64::new(1_000_000_000).unwrap());
        let gate = tree.add_gate(source, false).unwrap();
        assert_eq!(instruction_period(&tree, gate), Err(ClockError::Gated));
    }
}
