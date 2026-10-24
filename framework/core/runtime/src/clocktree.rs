//! Deterministic clock tree (設計書 §13.2, ADR 0015).
//!
//! A tree is a composition-root service: catalog models do not depend on it.
//! They receive a resolved period, or the driver converts instruction counts
//! through [`ClockTree::ticks_to_delta`]. Gating, integer division and mux
//! selection are explicit. Cross-domain jitter is not modeled.
//!
//! Nodes may only reference nodes that already exist, so construction cannot
//! form a cycle. Resolution still detects a cycle so a later rewire cannot
//! recurse forever. A disabled gate, and every node downstream of one, is
//! [`ClockState::Gated`]. A divider of a running clock uses integer division;
//! a result of zero hertz is an error, not a silent stall.

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use mosaic_contract_clock::{ClockState, PS_PER_SECOND};
use mosaic_core_types::{Fnv64, Snapshot, SnapshotError, VDelta};

/// Identity of one clock-tree node. Ids are allocated monotonically and are
/// not reused, so a checkpoint can name a node after other nodes are added
/// only when the destination tree has the same structure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ClockId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClockError {
    Unknown,
    Cycle,
    Gated,
    /// Divisor, mux index or frequency is not usable.
    InvalidParameter,
    /// The conversion is smaller than one picosecond, or an integer divide
    /// produced zero hertz. Functional time must not pretend that happened.
    Unrepresentable,
    Overflow,
    VersionMismatch,
    GraphMismatch,
    InvalidState,
}

impl From<ClockError> for SnapshotError {
    fn from(error: ClockError) -> Self {
        match error {
            ClockError::VersionMismatch => Self::VersionMismatch,
            ClockError::GraphMismatch => Self::GraphMismatch,
            ClockError::Unknown
            | ClockError::Cycle
            | ClockError::Gated
            | ClockError::InvalidParameter
            | ClockError::Unrepresentable
            | ClockError::Overflow
            | ClockError::InvalidState => Self::InvalidState,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Source {
        hz: NonZeroU64,
    },
    Divider {
        parent: ClockId,
        divisor: NonZeroU64,
    },
    Gate {
        parent: ClockId,
        enabled: bool,
    },
    Mux {
        parents: Vec<ClockId>,
        select: u32,
    },
}

const KIND_SOURCE: u8 = 1;
const KIND_DIVIDER: u8 = 2;
const KIND_GATE: u8 = 3;
const KIND_MUX: u8 = 4;

/// Structural identity of one node. Dynamic gate, mux and divisor values are
/// not part of the graph; they are checkpoint state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockStructureNode {
    pub id: u32,
    pub kind: u8,
    pub hz: u64,
    pub parents: Vec<u32>,
}

/// Versioned clock-tree checkpoint. Structure and dynamics travel together so
/// a restore can reject a different tree before applying gate state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockTreeState {
    pub schema: u32,
    pub nodes: Vec<ClockNodeState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockNodeState {
    pub id: u32,
    pub kind: u8,
    pub hz: u64,
    pub divisor: u64,
    pub enabled: bool,
    pub select: u32,
    pub parents: Vec<u32>,
}

/// Owned clock tree. The machine stores one; models never hold a reference.
#[derive(Clone, Debug, Default)]
pub struct ClockTree {
    nodes: BTreeMap<u32, Node>,
    next_id: u32,
}

impl ClockTree {
    pub const SCHEMA_VERSION: u32 = 1;
}

impl ClockTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn add_source(&mut self, hz: NonZeroU64) -> ClockId {
        self.insert(Node::Source { hz })
    }

    pub fn add_divider(
        &mut self,
        parent: ClockId,
        divisor: NonZeroU64,
    ) -> Result<ClockId, ClockError> {
        self.require(parent)?;
        Ok(self.insert(Node::Divider { parent, divisor }))
    }

    pub fn add_gate(&mut self, parent: ClockId, enabled: bool) -> Result<ClockId, ClockError> {
        self.require(parent)?;
        Ok(self.insert(Node::Gate { parent, enabled }))
    }

    pub fn add_mux(&mut self, parents: &[ClockId], select: u32) -> Result<ClockId, ClockError> {
        if parents.is_empty()
            || usize::try_from(select)
                .ok()
                .is_none_or(|index| index >= parents.len())
        {
            return Err(ClockError::InvalidParameter);
        }
        for parent in parents {
            self.require(*parent)?;
        }
        Ok(self.insert(Node::Mux {
            parents: parents.to_vec(),
            select,
        }))
    }

    /// Enable or close a gate. Returns the resolved output so the composition
    /// root can forward [`mosaic_contract_clock::ClockSink::clock_changed`]
    /// between callbacks. The tree does not call models itself.
    pub fn set_enabled(&mut self, id: ClockId, enabled: bool) -> Result<ClockState, ClockError> {
        let Node::Gate { enabled: slot, .. } =
            self.nodes.get_mut(&id.0).ok_or(ClockError::Unknown)?
        else {
            return Err(ClockError::InvalidParameter);
        };
        let previous = *slot;
        *slot = enabled;
        match self.resolve(id) {
            Ok(state) => Ok(state),
            Err(error) => {
                if let Some(Node::Gate { enabled: slot, .. }) = self.nodes.get_mut(&id.0) {
                    *slot = previous;
                }
                Err(error)
            }
        }
    }

    pub fn set_select(&mut self, id: ClockId, select: u32) -> Result<ClockState, ClockError> {
        let Node::Mux {
            parents,
            select: slot,
        } = self.nodes.get_mut(&id.0).ok_or(ClockError::Unknown)?
        else {
            return Err(ClockError::InvalidParameter);
        };
        if usize::try_from(select)
            .ok()
            .is_none_or(|index| index >= parents.len())
        {
            return Err(ClockError::InvalidParameter);
        }
        let previous = *slot;
        *slot = select;
        match self.resolve(id) {
            Ok(state) => Ok(state),
            Err(error) => {
                if let Some(Node::Mux { select: slot, .. }) = self.nodes.get_mut(&id.0) {
                    *slot = previous;
                }
                Err(error)
            }
        }
    }

    pub fn set_divisor(
        &mut self,
        id: ClockId,
        divisor: NonZeroU64,
    ) -> Result<ClockState, ClockError> {
        let Node::Divider { divisor: slot, .. } =
            self.nodes.get_mut(&id.0).ok_or(ClockError::Unknown)?
        else {
            return Err(ClockError::InvalidParameter);
        };
        let previous = *slot;
        *slot = divisor;
        match self.resolve(id) {
            Ok(state) => Ok(state),
            Err(error) => {
                if let Some(Node::Divider { divisor: slot, .. }) = self.nodes.get_mut(&id.0) {
                    *slot = previous;
                }
                Err(error)
            }
        }
    }

    pub fn resolve(&self, id: ClockId) -> Result<ClockState, ClockError> {
        let mut stack = Vec::new();
        self.resolve_inner(id, &mut stack)
    }

    /// Convert `ticks` of `id` into a virtual-time duration.
    ///
    /// The conversion is `(ticks * PS_PER_SECOND) / hz`, truncated once.
    /// Multiplying a per-tick period would accumulate that truncation.
    /// Zero ticks is a zero duration. A positive tick count that does not
    /// reach one picosecond is [`ClockError::Unrepresentable`].
    pub fn ticks_to_delta(&self, id: ClockId, ticks: u64) -> Result<VDelta, ClockError> {
        if ticks == 0 {
            return Ok(VDelta(0));
        }
        let ClockState::Running { hz } = self.resolve(id)? else {
            return Err(ClockError::Gated);
        };
        let picoseconds = u128::from(ticks)
            .checked_mul(u128::from(PS_PER_SECOND))
            .ok_or(ClockError::Overflow)?
            / u128::from(hz.get());
        let picoseconds = u64::try_from(picoseconds).map_err(|_| ClockError::Overflow)?;
        if picoseconds == 0 {
            return Err(ClockError::Unrepresentable);
        }
        Ok(VDelta(picoseconds))
    }

    /// Picoseconds of one tick, for schedulers that store a resolved period.
    ///
    /// A 1 GHz source resolves to 1000 ps, matching the historical multicore
    /// default. Sub-picosecond clocks cannot be used as an instruction period.
    pub fn period_ps(&self, id: ClockId) -> Result<NonZeroU64, ClockError> {
        let VDelta(picoseconds) = self.ticks_to_delta(id, 1)?;
        NonZeroU64::new(picoseconds).ok_or(ClockError::Unrepresentable)
    }

    pub(crate) fn structure(&self) -> Vec<ClockStructureNode> {
        self.nodes
            .iter()
            .map(|(&id, node)| match node {
                Node::Source { hz } => ClockStructureNode {
                    id,
                    kind: KIND_SOURCE,
                    hz: hz.get(),
                    parents: Vec::new(),
                },
                Node::Divider { parent, .. } => ClockStructureNode {
                    id,
                    kind: KIND_DIVIDER,
                    hz: 0,
                    parents: vec![parent.0],
                },
                Node::Gate { parent, .. } => ClockStructureNode {
                    id,
                    kind: KIND_GATE,
                    hz: 0,
                    parents: vec![parent.0],
                },
                Node::Mux { parents, .. } => ClockStructureNode {
                    id,
                    kind: KIND_MUX,
                    hz: 0,
                    parents: parents.iter().map(|parent| parent.0).collect(),
                },
            })
            .collect()
    }

    pub(crate) fn apply_dynamics(&mut self, state: &ClockTreeState) -> Result<(), ClockError> {
        if state.schema != Self::SCHEMA_VERSION {
            return Err(ClockError::VersionMismatch);
        }
        if !self.structure_matches(state) {
            return Err(ClockError::GraphMismatch);
        }
        let mut updated = self.nodes.clone();
        for node in &state.nodes {
            match updated.get_mut(&node.id).ok_or(ClockError::GraphMismatch)? {
                Node::Divider { divisor, .. } => {
                    *divisor = NonZeroU64::new(node.divisor).ok_or(ClockError::InvalidState)?;
                }
                Node::Gate { enabled, .. } => *enabled = node.enabled,
                Node::Mux { parents, select } => {
                    if usize::try_from(node.select)
                        .ok()
                        .is_none_or(|index| index >= parents.len())
                    {
                        return Err(ClockError::InvalidState);
                    }
                    *select = node.select;
                }
                Node::Source { .. } => {}
            }
        }
        self.nodes = updated;
        Ok(())
    }

    fn structure_matches(&self, state: &ClockTreeState) -> bool {
        let live = self.structure();
        if live.len() != state.nodes.len() {
            return false;
        }
        live.iter().zip(&state.nodes).all(|(live, saved)| {
            live.id == saved.id
                && live.kind == saved.kind
                && live.hz == saved.hz
                && live.parents == saved.parents
        })
    }

    fn insert(&mut self, node: Node) -> ClockId {
        let id = ClockId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        self.nodes.insert(id.0, node);
        id
    }

    fn require(&self, id: ClockId) -> Result<(), ClockError> {
        if self.nodes.contains_key(&id.0) {
            Ok(())
        } else {
            Err(ClockError::Unknown)
        }
    }

    fn resolve_inner(&self, id: ClockId, stack: &mut Vec<u32>) -> Result<ClockState, ClockError> {
        if stack.contains(&id.0) {
            return Err(ClockError::Cycle);
        }
        stack.push(id.0);
        let result = self.resolve_node(id, stack);
        stack.pop();
        result
    }

    fn resolve_node(&self, id: ClockId, stack: &mut Vec<u32>) -> Result<ClockState, ClockError> {
        match self.nodes.get(&id.0).ok_or(ClockError::Unknown)? {
            Node::Source { hz } => Ok(ClockState::Running { hz: *hz }),
            Node::Divider { parent, divisor } => match self.resolve_inner(*parent, stack)? {
                ClockState::Gated => Ok(ClockState::Gated),
                ClockState::Running { hz } => {
                    let divided = hz.get() / divisor.get();
                    let hz = NonZeroU64::new(divided).ok_or(ClockError::Unrepresentable)?;
                    Ok(ClockState::Running { hz })
                }
                // ClockState is non_exhaustive. An unknown variant cannot be a
                // period the rest of the runtime knows how to divide.
                _ => Err(ClockError::Unrepresentable),
            },
            Node::Gate { parent, enabled } => {
                if !enabled {
                    return Ok(ClockState::Gated);
                }
                self.resolve_inner(*parent, stack)
            }
            Node::Mux { parents, select } => {
                let index = usize::try_from(*select).map_err(|_| ClockError::InvalidParameter)?;
                let parent = parents
                    .get(index)
                    .copied()
                    .ok_or(ClockError::InvalidParameter)?;
                self.resolve_inner(parent, stack)
            }
        }
    }
}

impl Snapshot for ClockTree {
    type State = ClockTreeState;
    const SCHEMA_VERSION: u32 = ClockTree::SCHEMA_VERSION;

    fn save(&self) -> Self::State {
        ClockTreeState {
            schema: Self::SCHEMA_VERSION,
            nodes: self
                .nodes
                .iter()
                .map(|(&id, node)| match node {
                    Node::Source { hz } => ClockNodeState {
                        id,
                        kind: KIND_SOURCE,
                        hz: hz.get(),
                        divisor: 0,
                        enabled: false,
                        select: 0,
                        parents: Vec::new(),
                    },
                    Node::Divider { parent, divisor } => ClockNodeState {
                        id,
                        kind: KIND_DIVIDER,
                        hz: 0,
                        divisor: divisor.get(),
                        enabled: false,
                        select: 0,
                        parents: vec![parent.0],
                    },
                    Node::Gate { parent, enabled } => ClockNodeState {
                        id,
                        kind: KIND_GATE,
                        hz: 0,
                        divisor: 0,
                        enabled: *enabled,
                        select: 0,
                        parents: vec![parent.0],
                    },
                    Node::Mux { parents, select } => ClockNodeState {
                        id,
                        kind: KIND_MUX,
                        hz: 0,
                        divisor: 0,
                        enabled: false,
                        select: *select,
                        parents: parents.iter().map(|parent| parent.0).collect(),
                    },
                })
                .collect(),
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        if !self.structure_matches(state) {
            return Err(SnapshotError::GraphMismatch);
        }
        for node in &state.nodes {
            match node.kind {
                KIND_SOURCE => {}
                KIND_DIVIDER if node.divisor == 0 => return Err(SnapshotError::InvalidState),
                KIND_DIVIDER => {}
                KIND_GATE => {}
                KIND_MUX => {
                    let live = self
                        .nodes
                        .get(&node.id)
                        .ok_or(SnapshotError::GraphMismatch)?;
                    let Node::Mux { parents, .. } = live else {
                        return Err(SnapshotError::GraphMismatch);
                    };
                    if usize::try_from(node.select)
                        .ok()
                        .is_none_or(|index| index >= parents.len())
                    {
                        return Err(SnapshotError::InvalidState);
                    }
                }
                _ => return Err(SnapshotError::InvalidState),
            }
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.apply_dynamics(state)?;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.usize(state.nodes.len());
        for node in &state.nodes {
            hash.u32(node.id);
            hash.byte(node.kind);
            hash.u64(node.hz);
            hash.u64(node.divisor);
            hash.bool(node.enabled);
            hash.u32(node.select);
            hash.usize(node.parents.len());
            for parent in &node.parents {
                hash.u32(*parent);
            }
        }
        hash.finish()
    }
}

/// Hash only the structural fields. Used by the machine graph fingerprint so
/// a gate change does not look like a different machine.
pub(crate) fn hash_structure(hash: &mut Fnv64, nodes: &[ClockStructureNode]) {
    hash.usize(nodes.len());
    for node in nodes {
        hash.u32(node.id);
        hash.byte(node.kind);
        hash.u64(node.hz);
        hash.usize(node.parents.len());
        for parent in &node.parents {
            hash.u32(*parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract_clock::ClockState;

    fn mhz(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).unwrap()
    }

    #[test]
    fn one_gigahertz_is_the_historical_instruction_period() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(mhz(1_000_000_000));
        assert_eq!(tree.period_ps(source).unwrap().get(), 1000);
        assert_eq!(tree.ticks_to_delta(source, 4).unwrap(), VDelta(4000));
    }

    #[test]
    fn divider_gate_and_mux_resolve_without_cross_domain_jitter() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(mhz(24_000_000));
        let half = tree.add_divider(source, mhz(2)).unwrap();
        let gate = tree.add_gate(half, true).unwrap();
        let stopped = tree.add_source(mhz(32_768));
        let mux = tree.add_mux(&[gate, stopped], 0).unwrap();

        assert_eq!(
            tree.resolve(half).unwrap(),
            ClockState::Running {
                hz: mhz(12_000_000)
            }
        );
        assert_eq!(
            tree.period_ps(half).unwrap().get(),
            1_000_000_000_000 / 12_000_000
        );
        assert!(matches!(
            tree.resolve(mux).unwrap(),
            ClockState::Running { .. }
        ));

        assert!(matches!(
            tree.set_enabled(gate, false).unwrap(),
            ClockState::Gated
        ));
        assert_eq!(tree.resolve(mux).unwrap(), ClockState::Gated);
        assert_eq!(tree.ticks_to_delta(mux, 1), Err(ClockError::Gated));

        tree.set_select(mux, 1).unwrap();
        assert_eq!(
            tree.resolve(mux).unwrap(),
            ClockState::Running { hz: mhz(32_768) }
        );
    }

    #[test]
    fn a_rejected_update_does_not_commit() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(mhz(1_000));
        let divider = tree.add_divider(source, mhz(1)).unwrap();
        assert_eq!(
            tree.set_divisor(divider, mhz(10_000)),
            Err(ClockError::Unrepresentable)
        );
        assert_eq!(
            tree.resolve(divider).unwrap(),
            ClockState::Running { hz: mhz(1_000) }
        );
        assert_eq!(
            tree.set_select(source, 0),
            Err(ClockError::InvalidParameter)
        );
    }

    #[test]
    fn snapshot_restores_dynamics_and_rejects_a_different_tree() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(mhz(8_000_000));
        let gate = tree.add_gate(source, true).unwrap();
        let saved = tree.save();
        tree.set_enabled(gate, false).unwrap();
        tree.load(ClockTree::SCHEMA_VERSION, &saved).unwrap();
        assert!(matches!(
            tree.resolve(gate).unwrap(),
            ClockState::Running { .. }
        ));
        assert_eq!(
            ClockTree::fingerprint(ClockTree::SCHEMA_VERSION, &saved),
            ClockTree::fingerprint(ClockTree::SCHEMA_VERSION, &tree.save())
        );

        let mut other = ClockTree::new();
        other.add_source(mhz(8_000_000));
        assert_eq!(
            other.load(ClockTree::SCHEMA_VERSION, &saved),
            Err(SnapshotError::GraphMismatch)
        );
        assert_eq!(
            other.resolve(ClockId(0)).unwrap(),
            ClockState::Running { hz: mhz(8_000_000) }
        );
    }

    #[test]
    fn zero_ticks_do_not_invent_time_and_sub_picosecond_clocks_are_refused() {
        let mut tree = ClockTree::new();
        let source = tree.add_source(mhz(1_000_000_000));
        assert_eq!(tree.ticks_to_delta(source, 0).unwrap(), VDelta(0));
        let fast = tree.add_source(NonZeroU64::new(PS_PER_SECOND + 1).unwrap());
        assert_eq!(tree.period_ps(fast), Err(ClockError::Unrepresentable));
    }
}
