//! Host-owned RAM/ROM backing blocks for machine nodes.
//!
//! The symbolic [`crate::FlatView`] owns no storage. Each Ram/Rom mapping
//! resolves to a node id and a device-local offset; this store owns the byte
//! vector behind that node. Loader access may initialize Rom, guest access is
//! still gated by the view's permissions in the machine dispatch path.

use std::collections::BTreeMap;

use mosaic_core_types::{AccessSize, BusFault, Endian, Fnv64, NodeId, Snapshot, SnapshotError};

use crate::machine::MachineError;

/// Snapshot of every RAM/ROM backing block. Blocks are ordered by `NodeId`;
/// the byte vectors are owned copies and are not host pointers or dirty-page
/// deltas. The persistent encoding and page-level COW format are later work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackingState {
    pub blocks: Vec<BackingBlockState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackingBlockState {
    pub node: NodeId,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct BackingStore {
    blocks: BTreeMap<u32, Vec<u8>>,
}

impl BackingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_block(&mut self, node: NodeId, size: u64) -> Result<(), MachineError> {
        if size == 0 {
            return Err(MachineError::ZeroSizeBlock(node));
        }
        if self.blocks.contains_key(&node.0) {
            return Err(MachineError::DuplicateNode(node));
        }
        let length =
            usize::try_from(size).map_err(|_| MachineError::BackingTooLarge { node, size })?;
        self.blocks.insert(node.0, vec![0; length]);
        Ok(())
    }

    pub fn is_registered(&self, node: NodeId) -> bool {
        self.blocks.contains_key(&node.0)
    }

    pub fn len_of(&self, node: NodeId) -> Option<u64> {
        self.blocks.get(&node.0).map(|block| block.len() as u64)
    }

    /// Loader/debug initialization. Rom blocks can be written here; this path
    /// never relaxes guest permissions enforced by the view.
    pub fn loader_write(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), BusFault> {
        if bytes.is_empty() {
            return Ok(());
        }
        let block = self.blocks.get_mut(&node.0).ok_or(BusFault::Unmapped)?;
        let range = span(block.len(), offset, bytes.len())?;
        block[range].copy_from_slice(bytes);
        Ok(())
    }

    pub fn loader_read(&self, node: NodeId, offset: u64, bytes: &mut [u8]) -> Result<(), BusFault> {
        if bytes.is_empty() {
            return Ok(());
        }
        let block = self.blocks.get(&node.0).ok_or(BusFault::Unmapped)?;
        let range = span(block.len(), offset, bytes.len())?;
        bytes.copy_from_slice(&block[range]);
        Ok(())
    }

    pub fn guest_read(
        &self,
        node: NodeId,
        offset: u64,
        size: AccessSize,
        endian: Endian,
    ) -> Result<u64, BusFault> {
        let block = self.blocks.get(&node.0).ok_or(BusFault::Unmapped)?;
        let range = span(block.len(), offset, size.bytes().into())?;
        Ok(load(&block[range], endian))
    }

    pub fn guest_write(
        &mut self,
        node: NodeId,
        offset: u64,
        size: AccessSize,
        value: u64,
        endian: Endian,
    ) -> Result<(), BusFault> {
        let block = self.blocks.get_mut(&node.0).ok_or(BusFault::Unmapped)?;
        let range = span(block.len(), offset, size.bytes().into())?;
        store(&mut block[range], value, endian);
        Ok(())
    }

    /// Commit a state after it has passed `Snapshot::validate`.
    pub(crate) fn restore_validated(&mut self, state: &BackingState) {
        self.blocks = state
            .blocks
            .iter()
            .map(|block| (block.node.0, block.bytes.clone()))
            .collect();
    }

    pub(crate) fn block_sizes(&self) -> BTreeMap<u32, usize> {
        self.blocks
            .iter()
            .map(|(&node, bytes)| (node, bytes.len()))
            .collect()
    }
}

impl Snapshot for BackingStore {
    type State = BackingState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        BackingState {
            blocks: self
                .blocks
                .iter()
                .map(|(&node, bytes)| BackingBlockState {
                    node: NodeId(node),
                    bytes: bytes.clone(),
                })
                .collect(),
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        if state.blocks.len() != self.blocks.len() {
            return Err(SnapshotError::GraphMismatch);
        }
        let mut seen = BTreeMap::new();
        for block in &state.blocks {
            if seen.insert(block.node.0, block.bytes.len()).is_some() {
                return Err(SnapshotError::InvalidState);
            }
        }
        if seen != self.block_sizes() {
            return Err(SnapshotError::GraphMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.restore_validated(state);
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.usize(state.blocks.len());
        for block in &state.blocks {
            hash.u32(block.node.0);
            hash.usize(block.bytes.len());
            hash.bytes(&block.bytes);
        }
        hash.finish()
    }
}

fn span(len: usize, offset: u64, count: usize) -> Result<std::ops::Range<usize>, BusFault> {
    let end = u128::from(offset) + u128::from(count as u64);
    if end > len as u128 {
        return Err(BusFault::Unmapped);
    }
    let start = usize::try_from(offset).map_err(|_| BusFault::Unmapped)?;
    Ok(start..start + count)
}

fn load(bytes: &[u8], endian: Endian) -> u64 {
    let mut value = 0u64;
    match endian {
        Endian::Little => {
            for (index, byte) in bytes.iter().enumerate() {
                value |= u64::from(*byte) << (8 * index);
            }
        }
        Endian::Big => {
            for byte in bytes {
                value = (value << 8) | u64::from(*byte);
            }
        }
    }
    value
}

fn store(bytes: &mut [u8], value: u64, endian: Endian) {
    let len = bytes.len();
    for (index, slot) in bytes.iter_mut().enumerate() {
        let shift = match endian {
            Endian::Little => 8 * index,
            Endian::Big => 8 * (len - 1 - index),
        };
        *slot = ((value >> shift) & 0xff) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_and_loader_accesses_are_bounds_checked_without_panics() {
        let mut store = BackingStore::new();
        let node = NodeId(1);
        store.add_block(node, 8).unwrap();
        assert_eq!(
            store.guest_read(node, 0, AccessSize::DoubleWord, Endian::Little),
            Ok(0)
        );
        assert_eq!(
            store.guest_write(node, 6, AccessSize::Word, 0x1122_3344, Endian::Little),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            store.loader_write(node, 7, &[1, 2]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            store.guest_read(NodeId(9), 0, AccessSize::Byte, Endian::Little),
            Err(BusFault::Unmapped)
        );
        // Empty loader transfers are no-ops like the engine memory contract.
        assert_eq!(store.loader_write(NodeId(9), 0, &[]), Ok(()));
        assert_eq!(store.loader_read(NodeId(9), 0, &mut []), Ok(()));
    }

    #[test]
    fn endianness_is_honored_for_every_width() {
        let mut store = BackingStore::new();
        let node = NodeId(2);
        store.add_block(node, 16).unwrap();
        store
            .guest_write(node, 0, AccessSize::Word, 0x1122_3344, Endian::Little)
            .unwrap();
        let mut bytes = [0; 4];
        store.loader_read(node, 0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x44, 0x33, 0x22, 0x11]);
        assert_eq!(
            store.guest_read(node, 0, AccessSize::Word, Endian::Little),
            Ok(0x1122_3344)
        );
        store
            .guest_write(node, 4, AccessSize::HalfWord, 0xabcd, Endian::Big)
            .unwrap();
        store.loader_read(node, 4, &mut bytes[..2]).unwrap();
        assert_eq!(&bytes[..2], &[0xab, 0xcd]);
        assert_eq!(
            store.guest_read(node, 4, AccessSize::HalfWord, Endian::Big),
            Ok(0xabcd)
        );
        store
            .guest_write(node, 8, AccessSize::Byte, 0x1234, Endian::Little)
            .unwrap();
        assert_eq!(
            store.guest_read(node, 8, AccessSize::Byte, Endian::Little),
            Ok(0x34)
        );
    }

    #[test]
    fn blocks_reject_duplicates_and_zero_size() {
        let mut store = BackingStore::new();
        let node = NodeId(3);
        store.add_block(node, 4).unwrap();
        assert_eq!(
            store.add_block(node, 4),
            Err(MachineError::DuplicateNode(node))
        );
        assert!(matches!(
            store.add_block(NodeId(4), 0),
            Err(MachineError::ZeroSizeBlock(_))
        ));
        assert_eq!(store.len_of(node), Some(4));
    }
}
