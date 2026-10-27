use mosaic_exec::{AccessSize, Addr, BusFault, Mapping, MemoryKind, NodeId};

/// Higher-priority siblings overlay lower-priority siblings. Container offsets
/// are relative to their parent. Equal-priority sibling ranges must not overlap.
#[derive(Clone, Debug)]
pub struct Region {
    base: Addr,
    size: u64,
    priority: i32,
    kind: RegionKind,
}

#[derive(Clone, Debug)]
enum RegionKind {
    Container(Vec<Region>),
    Leaf(Mapping),
}

impl Region {
    pub fn leaf(mapping: Mapping, priority: i32) -> Self {
        Self {
            base: mapping.base,
            size: mapping.size,
            priority,
            kind: RegionKind::Leaf(mapping),
        }
    }

    pub fn container(base: Addr, size: u64, priority: i32, children: Vec<Self>) -> Self {
        Self {
            base,
            size,
            priority,
            kind: RegionKind::Container(children),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressError {
    Empty,
    Overflow,
    OutsideParent,
    AmbiguousOverlap,
    TooDeep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resolved {
    pub target: NodeId,
    pub offset: u64,
    pub kind: MemoryKind,
}

/// Immutable, sorted and non-overlapping symbolic map. This owns no RAM or
/// device references. Build a separate view for each CPU/security/DMA viewpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlatView {
    mappings: Vec<Mapping>,
}

fn end(base: u64, size: u64) -> u128 {
    u128::from(base) + u128::from(size)
}

fn overlay(mappings: &mut Vec<Mapping>, new: Mapping) {
    let mut split = Vec::new();
    for old in mappings.drain(..) {
        if end(old.base.0, old.size) <= u128::from(new.base.0)
            || end(new.base.0, new.size) <= u128::from(old.base.0)
        {
            split.push(old);
            continue;
        }
        if old.base < new.base {
            split.push(Mapping {
                size: new.base.0 - old.base.0,
                ..old
            });
        }
        if end(old.base.0, old.size) > end(new.base.0, new.size) {
            // This branch proves new's exclusive end fits u64 (it is < old end).
            let start = end(new.base.0, new.size) as u64;
            split.push(Mapping {
                base: Addr(start),
                size: (end(old.base.0, old.size) - u128::from(start)) as u64,
                offset: old.offset + (start - old.base.0),
                ..old
            });
        }
    }
    split.push(new);
    split.sort_by_key(|mapping| mapping.base);
    *mappings = split;
}

fn flatten(
    regions: &[Region],
    parent: u64,
    limit: Option<u64>,
    depth: usize,
) -> Result<Vec<Mapping>, AddressError> {
    if depth > 64 {
        return Err(AddressError::TooDeep);
    }
    for (i, region) in regions.iter().enumerate() {
        if region.size == 0 {
            return Err(AddressError::Empty);
        }
        if end(region.base.0, region.size) > (1u128 << 64) {
            return Err(AddressError::Overflow);
        }
        if limit.is_some_and(|size| end(region.base.0, region.size) > u128::from(size)) {
            return Err(AddressError::OutsideParent);
        }
        for other in &regions[..i] {
            if region.priority == other.priority
                && u128::from(region.base.0) < end(other.base.0, other.size)
                && u128::from(other.base.0) < end(region.base.0, region.size)
            {
                return Err(AddressError::AmbiguousOverlap);
            }
        }
    }
    let mut order: Vec<_> = regions.iter().collect();
    order.sort_by_key(|r| r.priority);
    let mut mappings = Vec::new();
    for region in order {
        let base = parent
            .checked_add(region.base.0)
            .ok_or(AddressError::Overflow)?;
        if end(base, region.size) > (1u128 << 64) {
            return Err(AddressError::Overflow);
        }
        let children = match &region.kind {
            RegionKind::Container(children) => {
                flatten(children, base, Some(region.size), depth + 1)?
            }
            RegionKind::Leaf(mapping) => {
                if end(mapping.offset, mapping.size) > (1u128 << 64) {
                    return Err(AddressError::Overflow);
                }
                vec![Mapping {
                    base: Addr(base),
                    ..*mapping
                }]
            }
        };
        for child in children {
            overlay(&mut mappings, child);
        }
    }
    Ok(mappings)
}

impl FlatView {
    pub fn build(regions: &[Region]) -> Result<Self, AddressError> {
        Ok(Self {
            mappings: flatten(regions, 0, None, 0)?,
        })
    }

    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }

    /// A transfer must fit a single flattened segment. Bus adapters must pass
    /// Attr unchanged when dispatching the resolved device-local access.
    pub fn resolve(
        &self,
        address: Addr,
        size: AccessSize,
        access: Access,
    ) -> Result<Resolved, BusFault> {
        let index = self
            .mappings
            .partition_point(|m| m.base <= address)
            .checked_sub(1)
            .ok_or(BusFault::Unmapped)?;
        let mapping = &self.mappings[index];
        if end(address.0, u64::from(size.bytes())) > end(mapping.base.0, mapping.size) {
            return Err(BusFault::Unmapped);
        }
        let allowed = match access {
            Access::Read => mapping.permissions.read,
            Access::Write => mapping.permissions.write,
            Access::Execute => mapping.permissions.execute,
        };
        if !allowed {
            return Err(BusFault::PermissionDenied);
        }
        Ok(Resolved {
            target: mapping.target,
            offset: mapping.offset + (address.0 - mapping.base.0),
            kind: mapping.kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::Permissions;

    fn leaf(base: u64, size: u64, node: u32, offset: u64, priority: i32) -> Region {
        Region::leaf(
            Mapping {
                base: Addr(base),
                size,
                target: NodeId(node),
                offset,
                kind: MemoryKind::Ram,
                permissions: Permissions::RWX,
            },
            priority,
        )
    }

    #[test]
    fn nested_offsets_and_device_local_offsets_are_distinct() {
        let view = FlatView::build(&[Region::container(
            Addr(0x1000),
            0x1000,
            0,
            vec![leaf(0x100, 0x100, 3, 0x20, 0)],
        )])
        .unwrap();
        assert_eq!(
            view.resolve(Addr(0x1104), AccessSize::Word, Access::Read),
            Ok(Resolved {
                target: NodeId(3),
                offset: 0x24,
                kind: MemoryKind::Ram
            })
        );
        assert_eq!(
            view.resolve(Addr(0x1000), AccessSize::Byte, Access::Read),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    fn high_priority_overlay_splits_and_preserves_backing_offsets() {
        let view = FlatView::build(&[leaf(0, 100, 1, 50, 0), leaf(20, 10, 2, 0, 1)]).unwrap();
        assert_eq!(
            view.mappings
                .iter()
                .map(|m| (m.base.0, m.size, m.offset))
                .collect::<Vec<_>>(),
            [(0, 20, 50), (20, 10, 0), (30, 70, 80)]
        );
        assert_eq!(
            view.resolve(Addr(30), AccessSize::Byte, Access::Read)
                .unwrap()
                .target,
            NodeId(1)
        );
        assert_eq!(
            view.resolve(Addr(29), AccessSize::HalfWord, Access::Read),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    fn container_holes_fall_through_to_lower_priority_siblings() {
        let view = FlatView::build(&[
            leaf(0, 100, 1, 0, 0),
            Region::container(Addr(10), 30, 1, vec![leaf(10, 10, 2, 0, 0)]),
        ])
        .unwrap();
        assert_eq!(
            view.resolve(Addr(15), AccessSize::Byte, Access::Read)
                .unwrap()
                .target,
            NodeId(1)
        );
        assert_eq!(
            view.resolve(Addr(20), AccessSize::Byte, Access::Read)
                .unwrap()
                .target,
            NodeId(2)
        );
    }

    #[test]
    fn construction_order_does_not_override_declared_priorities() {
        let a = leaf(0, 100, 1, 0, 0);
        let b = leaf(20, 10, 2, 0, 1);
        assert_eq!(
            FlatView::build(&[a.clone(), b.clone()]),
            FlatView::build(&[b, a])
        );
    }

    #[test]
    fn rejects_ambiguous_empty_out_of_parent_and_overflowing_regions() {
        assert_eq!(
            FlatView::build(&[leaf(0, 20, 1, 0, 0), leaf(10, 20, 2, 0, 0)]),
            Err(AddressError::AmbiguousOverlap)
        );
        assert_eq!(
            FlatView::build(&[leaf(0, 0, 1, 0, 0)]),
            Err(AddressError::Empty)
        );
        assert_eq!(
            FlatView::build(&[Region::container(Addr(0), 10, 0, vec![leaf(9, 2, 1, 0, 0)])]),
            Err(AddressError::OutsideParent)
        );
        assert_eq!(
            FlatView::build(&[leaf(u64::MAX, 2, 1, 0, 0)]),
            Err(AddressError::Overflow)
        );
        assert_eq!(
            FlatView::build(&[leaf(0, 2, 1, u64::MAX, 0)]),
            Err(AddressError::Overflow)
        );
    }

    #[test]
    fn top_address_byte_is_valid_but_a_cross_boundary_access_is_not() {
        let view = FlatView::build(&[leaf(u64::MAX, 1, 1, u64::MAX, 0)]).unwrap();
        assert_eq!(
            view.resolve(Addr(u64::MAX), AccessSize::Byte, Access::Read)
                .unwrap()
                .offset,
            u64::MAX
        );
        assert_eq!(
            view.resolve(Addr(u64::MAX), AccessSize::HalfWord, Access::Read),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    fn permissions_and_independent_views_are_enforced() {
        let mapping = Mapping {
            base: Addr(0),
            size: 4096,
            target: NodeId(1),
            offset: 0,
            kind: MemoryKind::Rom,
            permissions: Permissions::RX,
        };
        let first = FlatView::build(&[Region::leaf(mapping, 0)]).unwrap();
        let second = FlatView::build(&[leaf(0, 4096, 2, 0, 0)]).unwrap();
        assert!(first
            .resolve(Addr(0), AccessSize::Word, Access::Execute)
            .is_ok());
        assert_eq!(
            first.resolve(Addr(0), AccessSize::Word, Access::Write),
            Err(BusFault::PermissionDenied)
        );
        assert_ne!(
            first
                .resolve(Addr(0), AccessSize::Byte, Access::Read)
                .unwrap()
                .target,
            second
                .resolve(Addr(0), AccessSize::Byte, Access::Read)
                .unwrap()
                .target
        );
        assert_eq!(
            FlatView::build(&[])
                .unwrap()
                .resolve(Addr(0), AccessSize::Byte, Access::Read),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    fn excessive_nesting_is_rejected() {
        let mut region = leaf(0, 1, 0, 0, 0);
        for _ in 0..66 {
            region = Region::container(Addr(0), 1, 0, vec![region]);
        }
        assert_eq!(FlatView::build(&[region]), Err(AddressError::TooDeep));
    }
}
