//! Deterministic normalized graph derived from validated SoC/board schemas.

use std::collections::{BTreeMap, BTreeSet};

use crate::endpoint::parse_endpoint;
use crate::schema::{BoardManifest, ConfigValue, DescriptorError, RegionKind, SocManifest};

/// Normalized node kind after SoC and board descriptions are combined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphNodeKind {
    Cpu,
    Peripheral,
    AddressSpaceView,
    Part,
    MemoryBacking,
    ClockSignal,
    ExternalEndpoint,
}

/// A node in the normalized declarative graph. `id` is a stable local graph
/// key (`soc.cpu0`, `board.sensor0`, or `memory.ram`), not a runtime `NodeId`.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub kind: GraphNodeKind,
    pub catalog_id: Option<String>,
    pub parameters: BTreeMap<String, ConfigValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphRegion {
    pub name: String,
    /// Stable graph node ID for the address-space view (for example `view.sysbus`).
    pub view: String,
    pub target_node: String,
    pub base: u64,
    pub size: u64,
    pub kind: RegionKind,
    pub priority: Option<i32>,
}

/// Directed, named port connection. `parameters` carries model-specific
/// connection values (for example an I2C address or SPI chip select).
#[derive(Clone, Debug, PartialEq)]
pub struct GraphEdge {
    pub id: String,
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
    pub kind: String,
    pub parameters: BTreeMap<String, ConfigValue>,
}

/// Stable declarative intermediate representation used between file parsing
/// and runtime-specific assembly. It contains no runtime handles or NodeIds.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineGraph {
    pub soc_id: String,
    pub board_id: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub regions: Vec<GraphRegion>,
    pub edges: Vec<GraphEdge>,
}

impl MachineGraph {
    /// Validate and combine manifests into a deterministic graph. Catalog IDs
    /// are preserved but not resolved here; the catalog registry belongs to a
    /// higher layer and can report missing implementations independently.
    pub fn build(
        soc: &SocManifest,
        board: Option<&BoardManifest>,
    ) -> Result<Self, DescriptorError> {
        soc.validate()?;
        if let Some(board) = board {
            board.validate()?;
            if let Some(board_soc) = &board.board.soc {
                if board_soc != &soc.soc.id {
                    return Err(DescriptorError::new(
                        "board.soc",
                        format!(
                            "board {:?} targets {:?}, but supplied SoC is {:?}",
                            board.board.id, board_soc, soc.soc.id
                        ),
                    ));
                }
            } else {
                // A SoC-less board (a multi-chip machine, ADR 0023) contributes
                // no graph of its own; its glue crate assembles it. Supplying a
                // SoC alongside one is a caller error.
                return Err(DescriptorError::new(
                    "board.soc",
                    format!(
                        "board {:?} declares no SoC, so it has no graph to build \
                         with {:?}",
                        board.board.id, soc.soc.id
                    ),
                ));
            }
        }

        let mut nodes = BTreeMap::<String, GraphNode>::new();
        let mut edges = Vec::new();
        let mut regions = Vec::new();

        for cpu in &soc.cpu {
            let id = format!("soc.{}", cpu.name);
            insert_graph_node(
                &mut nodes,
                GraphNode {
                    id,
                    kind: GraphNodeKind::Cpu,
                    catalog_id: Some(cpu.use_id.clone()),
                    parameters: BTreeMap::new(),
                },
            )?;
        }
        for block in &soc.block {
            let id = format!("soc.{}", block.name);
            insert_graph_node(
                &mut nodes,
                GraphNode {
                    id,
                    kind: GraphNodeKind::Peripheral,
                    catalog_id: Some(block.use_id.clone()),
                    parameters: BTreeMap::new(),
                },
            )?;
        }
        for view in &soc.view {
            insert_graph_node(
                &mut nodes,
                GraphNode {
                    id: format!("view.{}", view.name),
                    kind: GraphNodeKind::AddressSpaceView,
                    catalog_id: None,
                    parameters: BTreeMap::new(),
                },
            )?;
            for region in &view.region {
                let target_node = if region.kind == RegionKind::Mmio {
                    format!("soc.{}", region.target)
                } else {
                    let id = format!("memory.{}", region.target);
                    if !nodes.contains_key(&id) {
                        insert_graph_node(
                            &mut nodes,
                            GraphNode {
                                id: id.clone(),
                                kind: GraphNodeKind::MemoryBacking,
                                catalog_id: None,
                                parameters: BTreeMap::new(),
                            },
                        )?;
                    }
                    id
                };
                regions.push(GraphRegion {
                    name: region.name.clone(),
                    view: format!("view.{}", view.name),
                    target_node,
                    base: region.base,
                    size: region.size,
                    kind: region.kind,
                    priority: region.priority,
                });
            }
        }

        for cpu in &soc.cpu {
            edges.push(GraphEdge {
                id: format!("view.{}.space", cpu.name),
                from_node: format!("soc.{}", cpu.name),
                from_port: "address-space".into(),
                to_node: format!("view.{}", cpu.view),
                to_port: String::new(),
                kind: "address-space".into(),
                parameters: BTreeMap::new(),
            });
        }

        // Clock sources may reference a peer block's output port by name
        // (`rcc.gate-apb2-iopa`), so collect the block names first (ADR 0025).
        let block_names: BTreeSet<&str> =
            soc.block.iter().map(|block| block.name.as_str()).collect();
        for block in &soc.block {
            for (input, source) in &block.clk {
                let source = source.as_str().ok_or_else(|| {
                    DescriptorError::new("block.clk", "clock source must be a string")
                })?;
                // A clock source is either an abstract clock net name
                // (`clk_hse`) or a device output port reference
                // (`rcc.gate-apb2-iopa`): the former gets an abstract clock
                // node, the latter wires the named block's output port
                // directly, the way a board wires a clock gate.
                let (from_node, from_port, edge_id) = match source.split_once('.') {
                    Some((device, port)) if block_names.contains(device) => (
                        format!("soc.{device}"),
                        port.to_owned(),
                        format!("clock.{}.{}.{}", block.name, input, port),
                    ),
                    _ => {
                        let clock_node = format!("clock.{source}");
                        if !nodes.contains_key(&clock_node) {
                            insert_graph_node(
                                &mut nodes,
                                GraphNode {
                                    id: clock_node.clone(),
                                    kind: GraphNodeKind::ClockSignal,
                                    catalog_id: None,
                                    parameters: BTreeMap::new(),
                                },
                            )?;
                        }
                        (
                            clock_node,
                            "out".into(),
                            format!("clock.{}.{}", block.name, input),
                        )
                    }
                };
                edges.push(GraphEdge {
                    id: edge_id,
                    from_node,
                    from_port,
                    to_node: format!("soc.{}", block.name),
                    to_port: input.clone(),
                    kind: "clock".into(),
                    parameters: BTreeMap::new(),
                });
            }
            for irq in &block.irq {
                for (index, target) in irq.to.iter().enumerate() {
                    let endpoint = parse_endpoint(target, "soc")?;
                    if !nodes.contains_key(&endpoint.node_id) {
                        return Err(DescriptorError::new(
                            "block.irq.to",
                            format!(
                                "block {:?} targets unknown graph node {:?}",
                                block.name, endpoint.node_id
                            ),
                        ));
                    }
                    edges.push(GraphEdge {
                        id: format!("irq.{}.{}.{}", block.name, irq.pin, index),
                        from_node: format!("soc.{}", block.name),
                        from_port: irq.pin.clone(),
                        to_node: endpoint.node_id,
                        to_port: endpoint.port,
                        kind: "irq".into(),
                        parameters: BTreeMap::new(),
                    });
                }
            }
        }

        if let Some(board) = board {
            for part in &board.part {
                insert_graph_node(
                    &mut nodes,
                    GraphNode {
                        id: format!("board.{}", part.name),
                        kind: GraphNodeKind::Part,
                        catalog_id: Some(part.use_id.clone()),
                        parameters: BTreeMap::new(),
                    },
                )?;
            }
            // Face wiring (ADR 0032): each `[[part.face]]` becomes one edge
            // whose direction follows the face kind. Parts may be wired onto
            // several fabrics at once — the faces are the wiring, not the
            // model's directory.
            let block_names: BTreeSet<&str> =
                soc.block.iter().map(|block| block.name.as_str()).collect();
            for part in &board.part {
                for face in &part.faces {
                    let part_node = format!("board.{}", part.name);
                    match face.kind.as_str() {
                        "i2c" | "spi" => {
                            let endpoint = parse_endpoint(&face.to, "soc")?;
                            if !nodes.contains_key(&endpoint.node_id) {
                                return Err(DescriptorError::new(
                                    "part.face.to",
                                    format!(
                                        "face {:?} of part {:?} references unknown initiator \
                                         node {:?}",
                                        face.port, part.name, endpoint.node_id
                                    ),
                                ));
                            }
                            edges.push(GraphEdge {
                                id: format!("face.{}.{}", part.name, face.port),
                                from_node: endpoint.node_id,
                                from_port: endpoint.port,
                                to_node: part_node,
                                to_port: face.port.clone(),
                                kind: face.kind.clone(),
                                parameters: face.parameters.clone(),
                            });
                        }
                        "gpio" => {
                            let index = face.index.unwrap_or(0);
                            ensure_external_node(&mut nodes, &face.to)?;
                            if !nodes.contains_key(&face.to) {
                                return Err(DescriptorError::new(
                                    "part.face.to",
                                    format!(
                                        "gpio face {:?} of part {:?} references unknown net {:?}",
                                        face.port, part.name, face.to
                                    ),
                                ));
                            }
                            let mut parameters = face.parameters.clone();
                            parameters.insert("index".into(), ConfigValue::Integer(index as i64));
                            edges.push(GraphEdge {
                                id: format!("face.{}.{}.{}", part.name, face.port, index),
                                from_node: part_node,
                                from_port: face.port.clone(),
                                to_node: face.to.clone(),
                                to_port: String::new(),
                                kind: "gpio".into(),
                                parameters,
                            });
                        }
                        "irq" => {
                            let Some(pin) = &face.pin else {
                                return Err(DescriptorError::new(
                                    "part.face.pin",
                                    format!(
                                        "irq face of part {:?} must declare the output pin name",
                                        part.name
                                    ),
                                ));
                            };
                            let endpoint = parse_endpoint(&face.to, "soc")?;
                            ensure_external_node(&mut nodes, &endpoint.node_id)?;
                            if !nodes.contains_key(&endpoint.node_id) {
                                return Err(DescriptorError::new(
                                    "part.face.to",
                                    format!(
                                        "irq face {:?} of part {:?} references unknown sink \
                                         node {:?}",
                                        face.port, part.name, endpoint.node_id
                                    ),
                                ));
                            }
                            edges.push(GraphEdge {
                                id: format!("face.{}.{}.{}", part.name, face.port, pin),
                                from_node: part_node,
                                from_port: pin.clone(),
                                to_node: endpoint.node_id,
                                to_port: endpoint.port,
                                kind: "irq".into(),
                                parameters: face.parameters.clone(),
                            });
                        }
                        "clock" => {
                            // A clock source is either a peer block's output
                            // port (`rcc.gate-apb1-i2c1`) or an abstract
                            // clock net name — the same two shapes a block's
                            // `clk` accepts.
                            let (from_node, from_port) = match face.to.split_once('.') {
                                Some((device, port)) if block_names.contains(device) => {
                                    (format!("soc.{device}"), port.to_owned())
                                }
                                _ => {
                                    let clock_node = format!("clock.{}", face.to);
                                    if !nodes.contains_key(&clock_node) {
                                        insert_graph_node(
                                            &mut nodes,
                                            GraphNode {
                                                id: clock_node.clone(),
                                                kind: GraphNodeKind::ClockSignal,
                                                catalog_id: None,
                                                parameters: BTreeMap::new(),
                                            },
                                        )?;
                                    }
                                    (clock_node, "out".into())
                                }
                            };
                            edges.push(GraphEdge {
                                id: format!("face.{}.{}", part.name, face.port),
                                from_node,
                                from_port,
                                to_node: part_node,
                                to_port: face.port.clone(),
                                kind: "clock".into(),
                                parameters: face.parameters.clone(),
                            });
                        }
                        other => {
                            return Err(DescriptorError::new(
                                "part.face.kind",
                                format!(
                                    "part {:?} uses unsupported face kind {other:?}",
                                    part.name
                                ),
                            ));
                        }
                    }
                }
            }
            for connection in &board.conn {
                let source = parse_endpoint(&connection.port, "soc")?;
                let target = parse_endpoint(&connection.to, "soc")?;
                ensure_external_node(&mut nodes, &source.node_id)?;
                if !nodes.contains_key(&source.node_id) {
                    return Err(DescriptorError::new(
                        "conn.port",
                        format!(
                            "connection {:?} references unknown source node {:?}",
                            connection.name, source.node_id
                        ),
                    ));
                }
                ensure_external_node(&mut nodes, &target.node_id)?;
                if !nodes.contains_key(&target.node_id) {
                    return Err(DescriptorError::new(
                        "conn.to",
                        format!(
                            "connection {:?} references unknown target node {:?}",
                            connection.name, target.node_id
                        ),
                    ));
                }
                edges.push(GraphEdge {
                    id: format!("board.{}", connection.name),
                    from_node: source.node_id,
                    from_port: source.port,
                    to_node: target.node_id,
                    to_port: target.port,
                    kind: connection.kind.clone(),
                    parameters: BTreeMap::new(),
                });
            }
        }

        let nodes: Vec<_> = nodes.into_values().collect();
        // Canonicalize the public IR so file ordering does not leak into graph
        // diffs or future graph fingerprints. Indexed IRQ edges remain ordered
        // by their explicit edge ID, not by a hash-map iteration order.
        regions.sort_by(|left, right| {
            (&left.view, left.base, &left.name).cmp(&(&right.view, right.base, &right.name))
        });
        edges.sort_by(|left, right| left.id.cmp(&right.id));
        validate_graph(&nodes, &regions, &edges)?;

        Ok(Self {
            soc_id: soc.soc.id.clone(),
            board_id: board.map(|value| value.board.id.clone()),
            nodes,
            regions,
            edges,
        })
    }
}

/// Check integrity of the normalized representation, not just its source
/// manifests. Keeping this pass separate makes future graph transformations
/// subject to the same endpoint/region invariants.
pub(crate) fn validate_graph(
    nodes: &[GraphNode],
    regions: &[GraphRegion],
    edges: &[GraphEdge],
) -> Result<(), DescriptorError> {
    let mut by_id = BTreeMap::new();
    for node in nodes {
        if node.id.trim().is_empty() {
            return Err(DescriptorError::new(
                "graph.node.id",
                "node ID must not be empty",
            ));
        }
        if by_id.insert(node.id.as_str(), node).is_some() {
            return Err(DescriptorError::new(
                "graph.node.id",
                format!("duplicate graph node ID {:?}", node.id),
            ));
        }
    }
    let mut edge_ids = BTreeSet::new();
    for edge in edges {
        if !edge_ids.insert(edge.id.as_str()) {
            return Err(DescriptorError::new(
                "graph.edge.id",
                format!("duplicate graph edge ID {:?}", edge.id),
            ));
        }
        if !by_id.contains_key(edge.from_node.as_str()) {
            return Err(DescriptorError::new(
                "graph.edge.from",
                format!(
                    "edge {:?} references missing node {:?}",
                    edge.id, edge.from_node
                ),
            ));
        }
        if !by_id.contains_key(edge.to_node.as_str()) {
            return Err(DescriptorError::new(
                "graph.edge.to",
                format!(
                    "edge {:?} references missing node {:?}",
                    edge.id, edge.to_node
                ),
            ));
        }
    }
    let mut region_names = BTreeSet::new();
    for (index, region) in regions.iter().enumerate() {
        if region.size == 0 || u128::from(region.base) + u128::from(region.size) > (1u128 << 64) {
            return Err(DescriptorError::new(
                "graph.region.range",
                format!("region {:?} has an invalid address range", region.name),
            ));
        }
        if !region_names.insert((region.view.as_str(), region.name.as_str())) {
            return Err(DescriptorError::new(
                "graph.region.name",
                format!(
                    "duplicate region {:?} in view {:?}",
                    region.name, region.view
                ),
            ));
        }
        for previous in &regions[..index] {
            let overlap = previous.view == region.view
                && previous.priority.unwrap_or(0) == region.priority.unwrap_or(0)
                && u128::from(region.base) < u128::from(previous.base) + u128::from(previous.size)
                && u128::from(previous.base) < u128::from(region.base) + u128::from(region.size);
            if overlap {
                return Err(DescriptorError::new(
                    "graph.region.overlap",
                    format!(
                        "regions {:?} and {:?} overlap at equal priority",
                        previous.name, region.name
                    ),
                ));
            }
        }
        let view = by_id.get(region.view.as_str()).ok_or_else(|| {
            DescriptorError::new(
                "graph.region.view",
                format!(
                    "region {:?} references missing view {:?}",
                    region.name, region.view
                ),
            )
        })?;
        if view.kind != GraphNodeKind::AddressSpaceView {
            return Err(DescriptorError::new(
                "graph.region.view",
                format!(
                    "region {:?} owner {:?} is not an address-space view",
                    region.name, region.view
                ),
            ));
        }
        let target = by_id.get(region.target_node.as_str()).ok_or_else(|| {
            DescriptorError::new(
                "graph.region.target",
                format!(
                    "region {:?} references missing node {:?}",
                    region.name, region.target_node
                ),
            )
        })?;
        let expected = match region.kind {
            RegionKind::Ram | RegionKind::Rom => GraphNodeKind::MemoryBacking,
            RegionKind::Mmio => GraphNodeKind::Peripheral,
        };
        if target.kind != expected {
            return Err(DescriptorError::new(
                "graph.region.target",
                format!(
                    "region {:?} targets {:?}, expected {:?}",
                    region.name, target.id, expected
                ),
            ));
        }
    }
    Ok(())
}

fn insert_graph_node(
    nodes: &mut BTreeMap<String, GraphNode>,
    node: GraphNode,
) -> Result<(), DescriptorError> {
    let id = node.id.clone();
    match nodes.entry(id.clone()) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(node);
            Ok(())
        }
        std::collections::btree_map::Entry::Occupied(_) => Err(DescriptorError::new(
            "graph.node",
            format!("duplicate graph node {id:?}"),
        )),
    }
}

fn ensure_external_node(
    nodes: &mut BTreeMap<String, GraphNode>,
    id: &str,
) -> Result<(), DescriptorError> {
    if id.starts_with("wire:") && !nodes.contains_key(id) {
        insert_graph_node(
            nodes,
            GraphNode {
                id: id.to_owned(),
                kind: GraphNodeKind::ExternalEndpoint,
                catalog_id: None,
                parameters: BTreeMap::new(),
            },
        )?;
    }
    Ok(())
}
