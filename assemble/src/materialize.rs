//! Turn a normalized [`MachineGraph`] into a runtime [`Machine`].
//!
//! This crate does not know catalog types. The composition root registers a
//! [`DeviceFactory`] per catalog ID (ADR 0016). IRQ fan-out is rejected rather
//! than silently dropped: the runtime wiring table is one sink per output.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mosaic_describe::{GraphNodeKind, MachineGraph, RegionKind};
use mosaic_exec::{Addr, Mapping, MemoryKind, NodeId, Permissions, SignalId};
use mosaic_runtime::{AddressError, FlatView, Machine, MachineError, Region};

/// Why a declared machine could not be instantiated.
#[derive(Debug)]
pub enum MaterializeError {
    DuplicateFactory { catalog_id: String },
    MissingFactory { catalog_id: String, node: String },
    MissingNode { node: String },
    MissingView { cpu: String, view: String },
    MissingOutput { node: String, port: String },
    Fanout { node: String, port: String },
    UnsupportedSink { edge: String, node: String },
    MultipleViews,
    Address(AddressError),
    Machine(MachineError),
    Bind { node: String, detail: String },
}

impl fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateFactory { catalog_id } => {
                write!(f, "catalog id {catalog_id:?} is already registered")
            }
            Self::MissingFactory { catalog_id, node } => {
                write!(f, "node {node:?} has no factory for {catalog_id:?}")
            }
            Self::MissingNode { node } => write!(f, "graph node {node:?} was not allocated"),
            Self::MissingView { cpu, view } => {
                write!(f, "cpu {cpu:?} selects missing view {view:?}")
            }
            Self::MissingOutput { node, port } => {
                write!(f, "node {node:?} has no assigned output {port:?}")
            }
            Self::Fanout { node, port } => write!(
                f,
                "output {node:?}.{port:?} drives more than one sink; runtime wiring is one-to-one"
            ),
            Self::UnsupportedSink { edge, node } => {
                write!(f, "irq edge {edge:?} targets unsupported node {node:?}")
            }
            Self::MultipleViews => write!(f, "a machine with no cpu cannot choose among views"),
            Self::Address(error) => write!(f, "address map: {error:?}"),
            Self::Machine(error) => write!(f, "machine: {error:?}"),
            Self::Bind { node, detail } => write!(f, "bind {node:?}: {detail}"),
        }
    }
}

impl From<AddressError> for MaterializeError {
    fn from(error: AddressError) -> Self {
        Self::Address(error)
    }
}

impl From<MachineError> for MaterializeError {
    fn from(error: MachineError) -> Self {
        Self::Machine(error)
    }
}

/// One incoming signal edge, already resolved to runtime identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncomingWire {
    pub port: String,
    pub source: NodeId,
    pub signal: SignalId,
}

impl IncomingWire {
    /// Numeric suffix of `inputs:37`. Named pins such as `irq:irq` return none.
    pub fn index(&self) -> Option<usize> {
        self.port.rsplit_once(':')?.1.parse().ok()
    }
}

/// What a factory may observe while constructing one node. Borrows die when
/// `instantiate` returns; do not store them on the model.
#[derive(Debug)]
pub struct InstantiateCtx<'a> {
    pub graph_id: &'a str,
    pub catalog_id: &'a str,
    pub node: NodeId,
    pub nodes: &'a BTreeMap<String, NodeId>,
    pub outputs: &'a BTreeMap<String, SignalId>,
    pub incoming: &'a [IncomingWire],
}

impl InstantiateCtx<'_> {
    pub fn output(&self, port: &str) -> Result<SignalId, MaterializeError> {
        self.outputs
            .get(port)
            .copied()
            .ok_or_else(|| MaterializeError::MissingOutput {
                node: self.graph_id.to_owned(),
                port: port.to_owned(),
            })
    }

    pub fn node(&self, id: &str) -> Result<NodeId, MaterializeError> {
        self.nodes
            .get(id)
            .copied()
            .ok_or_else(|| MaterializeError::MissingNode {
                node: id.to_owned(),
            })
    }
}

/// Catalog-ID lookup supplied by the composition root. Assemble never imports
/// a catalog crate.
pub trait DeviceFactory: Send + Sync {
    fn instantiate(
        &self,
        ctx: &InstantiateCtx,
        machine: &mut Machine,
    ) -> Result<(), MaterializeError>;
}

impl<F> DeviceFactory for F
where
    F: Fn(&InstantiateCtx, &mut Machine) -> Result<(), MaterializeError> + Send + Sync,
{
    fn instantiate(
        &self,
        ctx: &InstantiateCtx,
        machine: &mut Machine,
    ) -> Result<(), MaterializeError> {
        self(ctx, machine)
    }
}

/// Registry of catalog ID → factory. Missing IDs fail at materialize time.
#[derive(Default)]
pub struct DeviceRegistry {
    factories: BTreeMap<String, Box<dyn DeviceFactory>>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self {
            factories: BTreeMap::new(),
        }
    }

    pub fn register(
        &mut self,
        catalog_id: impl Into<String>,
        factory: impl DeviceFactory + 'static,
    ) -> Result<(), MaterializeError> {
        let catalog_id = catalog_id.into();
        if self.factories.contains_key(&catalog_id) {
            return Err(MaterializeError::DuplicateFactory { catalog_id });
        }
        self.factories.insert(catalog_id, Box::new(factory));
        Ok(())
    }
}

/// An IRQ edge whose sink is a CPU node, not a device. The composition root
/// forwards it through the engine mailbox (ADR 0009).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuLine {
    pub cpu: String,
    pub port: String,
    pub source: NodeId,
    pub signal: SignalId,
}

/// An edge this materializer does not wire. Presence here is not success.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredEdge {
    pub id: String,
    pub kind: String,
}

/// A machine whose declared MMIO, memory and one-to-one IRQ edges exist.
pub struct AssembledMachine {
    pub machine: Machine,
    pub view: FlatView,
    pub nodes: BTreeMap<String, NodeId>,
    /// `(graph node id, output port)` → assigned signal.
    pub outputs: BTreeMap<(String, String), SignalId>,
    pub cpu_lines: Vec<CpuLine>,
    pub deferred: Vec<DeferredEdge>,
    /// First `NodeId` not used by the graph. Host sinks may start here.
    pub next_node: u32,
}

impl fmt::Debug for AssembledMachine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssembledMachine")
            .field("nodes", &self.nodes)
            .field("outputs", &self.outputs)
            .field("cpu_lines", &self.cpu_lines)
            .field("deferred", &self.deferred)
            .field("next_node", &self.next_node)
            .finish_non_exhaustive()
    }
}

/// Instantiate `graph` with the factories in `registry`.
///
/// Node IDs are assigned in graph-id order so the same declaration always
/// produces the same runtime identities. Output signals start at 1, per source
/// node, in port-name order.
pub fn materialize(
    graph: &MachineGraph,
    registry: &DeviceRegistry,
    queue_capacity: usize,
) -> Result<AssembledMachine, MaterializeError> {
    let nodes = allocate_nodes(graph)?;
    let assigned = assign_irq_outputs(graph)?;
    let incoming = incoming_wires(graph, &nodes, &assigned)?;
    let views = build_views(graph, &nodes)?;
    let view = default_view(graph, &views)?;
    let mut machine = Machine::new(view.clone(), queue_capacity);
    add_backings(graph, &nodes, &mut machine)?;
    instantiate(graph, registry, &nodes, &assigned, &incoming, &mut machine)?;
    let cpu_lines = connect_irq(graph, &nodes, &assigned, &mut machine)?;
    attach_cpu_views(graph, &nodes, &views, &mut machine)?;
    machine.verify()?;

    let mut outputs = BTreeMap::new();
    for (node, ports) in &assigned {
        for (port, signal) in ports {
            outputs.insert((node.clone(), port.clone()), *signal);
        }
    }
    let deferred = graph
        .edges
        .iter()
        .filter(|edge| !matches!(edge.kind.as_str(), "irq" | "address-space"))
        .map(|edge| DeferredEdge {
            id: edge.id.clone(),
            kind: edge.kind.clone(),
        })
        .collect();
    let next_node = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
    Ok(AssembledMachine {
        machine,
        view,
        nodes,
        outputs,
        cpu_lines,
        deferred,
        next_node,
    })
}

fn allocates(kind: GraphNodeKind) -> bool {
    matches!(
        kind,
        GraphNodeKind::Cpu
            | GraphNodeKind::Peripheral
            | GraphNodeKind::Part
            | GraphNodeKind::MemoryBacking
            | GraphNodeKind::ExternalEndpoint
    )
}

fn allocate_nodes(graph: &MachineGraph) -> Result<BTreeMap<String, NodeId>, MaterializeError> {
    let mut names: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| allocates(node.kind))
        .map(|node| node.id.as_str())
        .collect();
    names.sort_unstable();
    let mut nodes = BTreeMap::new();
    for name in names {
        let id = u32::try_from(nodes.len()).map_err(|_| MaterializeError::MissingNode {
            node: name.to_owned(),
        })?;
        nodes.insert(name.to_owned(), NodeId(id));
    }
    Ok(nodes)
}

fn assign_irq_outputs(
    graph: &MachineGraph,
) -> Result<BTreeMap<String, BTreeMap<String, SignalId>>, MaterializeError> {
    let mut seen = BTreeSet::new();
    let mut ports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in graph.edges.iter().filter(|edge| edge.kind == "irq") {
        if !seen.insert((edge.from_node.as_str(), edge.from_port.as_str())) {
            return Err(MaterializeError::Fanout {
                node: edge.from_node.clone(),
                port: edge.from_port.clone(),
            });
        }
        ports
            .entry(edge.from_node.clone())
            .or_default()
            .insert(edge.from_port.clone());
    }
    let mut assigned = BTreeMap::new();
    for (node, names) in ports {
        let mut signals = BTreeMap::new();
        for (index, name) in names.into_iter().enumerate() {
            let id = u32::try_from(index + 1).unwrap_or(u32::MAX);
            signals.insert(name, SignalId(id));
        }
        assigned.insert(node, signals);
    }
    Ok(assigned)
}

fn incoming_wires(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
    assigned: &BTreeMap<String, BTreeMap<String, SignalId>>,
) -> Result<BTreeMap<String, Vec<IncomingWire>>, MaterializeError> {
    let mut incoming: BTreeMap<String, Vec<IncomingWire>> = BTreeMap::new();
    for edge in graph.edges.iter().filter(|edge| edge.kind == "irq") {
        let source = *nodes
            .get(&edge.from_node)
            .ok_or_else(|| MaterializeError::MissingNode {
                node: edge.from_node.clone(),
            })?;
        let signal = assigned
            .get(&edge.from_node)
            .and_then(|ports| ports.get(&edge.from_port))
            .copied()
            .ok_or_else(|| MaterializeError::MissingOutput {
                node: edge.from_node.clone(),
                port: edge.from_port.clone(),
            })?;
        incoming
            .entry(edge.to_node.clone())
            .or_default()
            .push(IncomingWire {
                port: edge.to_port.clone(),
                source,
                signal,
            });
    }
    for wires in incoming.values_mut() {
        wires.sort_by(|left, right| {
            (&left.port, left.source.0, left.signal.0).cmp(&(
                &right.port,
                right.source.0,
                right.signal.0,
            ))
        });
    }
    Ok(incoming)
}

fn build_views(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
) -> Result<BTreeMap<String, FlatView>, MaterializeError> {
    let mut grouped: BTreeMap<String, Vec<Region>> = BTreeMap::new();
    for region in &graph.regions {
        let target =
            *nodes
                .get(&region.target_node)
                .ok_or_else(|| MaterializeError::MissingNode {
                    node: region.target_node.clone(),
                })?;
        let (kind, permissions) = match region.kind {
            RegionKind::Ram => (MemoryKind::Ram, Permissions::RW),
            RegionKind::Rom => (MemoryKind::Rom, Permissions::RX),
            RegionKind::Mmio => (MemoryKind::Mmio, Permissions::RW),
        };
        grouped
            .entry(region.view.clone())
            .or_default()
            .push(Region::leaf(
                Mapping {
                    base: Addr(region.base),
                    size: region.size,
                    target,
                    offset: 0,
                    kind,
                    permissions,
                },
                region.priority.unwrap_or(0),
            ));
    }
    grouped
        .into_iter()
        .map(|(name, regions)| Ok((name, FlatView::build(&regions)?)))
        .collect()
}

fn default_view(
    graph: &MachineGraph,
    views: &BTreeMap<String, FlatView>,
) -> Result<FlatView, MaterializeError> {
    if let Some(cpu) = graph
        .nodes
        .iter()
        .filter(|node| node.kind == GraphNodeKind::Cpu)
        .min_by(|left, right| left.id.cmp(&right.id))
    {
        let view_id = cpu_view(graph, &cpu.id)?;
        return views
            .get(&view_id)
            .cloned()
            .ok_or_else(|| MaterializeError::MissingView {
                cpu: cpu.id.clone(),
                view: view_id,
            });
    }
    match views.len() {
        0 => Ok(FlatView::build(&[])?),
        1 => views
            .values()
            .next()
            .cloned()
            .ok_or(MaterializeError::MultipleViews),
        _ => Err(MaterializeError::MultipleViews),
    }
}

fn cpu_view(graph: &MachineGraph, cpu: &str) -> Result<String, MaterializeError> {
    graph
        .edges
        .iter()
        .find(|edge| {
            edge.kind == "address-space" && edge.from_node == cpu && !edge.to_node.is_empty()
        })
        .map(|edge| edge.to_node.clone())
        .ok_or_else(|| MaterializeError::MissingView {
            cpu: cpu.to_owned(),
            view: String::new(),
        })
}

fn add_backings(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let mut sizes: BTreeMap<&str, u64> = BTreeMap::new();
    for region in &graph.regions {
        if region.kind == RegionKind::Mmio {
            continue;
        }
        let size = sizes.entry(region.target_node.as_str()).or_insert(0);
        *size = (*size).max(region.size);
    }
    for (name, size) in sizes {
        let node = *nodes
            .get(name)
            .ok_or_else(|| MaterializeError::MissingNode {
                node: name.to_owned(),
            })?;
        machine.add_backing(node, size)?;
    }
    Ok(())
}

fn instantiate(
    graph: &MachineGraph,
    registry: &DeviceRegistry,
    nodes: &BTreeMap<String, NodeId>,
    assigned: &BTreeMap<String, BTreeMap<String, SignalId>>,
    incoming: &BTreeMap<String, Vec<IncomingWire>>,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let empty_outputs = BTreeMap::new();
    let empty_incoming = Vec::new();
    for node in &graph.nodes {
        if !matches!(node.kind, GraphNodeKind::Peripheral | GraphNodeKind::Part) {
            continue;
        }
        let catalog_id =
            node.catalog_id
                .as_deref()
                .ok_or_else(|| MaterializeError::MissingFactory {
                    catalog_id: String::new(),
                    node: node.id.clone(),
                })?;
        let factory =
            registry
                .factories
                .get(catalog_id)
                .ok_or_else(|| MaterializeError::MissingFactory {
                    catalog_id: catalog_id.to_owned(),
                    node: node.id.clone(),
                })?;
        let ctx = InstantiateCtx {
            graph_id: &node.id,
            catalog_id,
            node: nodes[&node.id],
            nodes,
            outputs: assigned.get(&node.id).unwrap_or(&empty_outputs),
            incoming: incoming.get(&node.id).unwrap_or(&empty_incoming),
        };
        factory.instantiate(&ctx, machine)?;
    }
    Ok(())
}

fn connect_irq(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
    assigned: &BTreeMap<String, BTreeMap<String, SignalId>>,
    machine: &mut Machine,
) -> Result<Vec<CpuLine>, MaterializeError> {
    let kinds: BTreeMap<&str, GraphNodeKind> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind))
        .collect();
    let mut cpu_lines = Vec::new();
    for edge in graph.edges.iter().filter(|edge| edge.kind == "irq") {
        let source = *nodes
            .get(&edge.from_node)
            .ok_or_else(|| MaterializeError::MissingNode {
                node: edge.from_node.clone(),
            })?;
        let signal = assigned
            .get(&edge.from_node)
            .and_then(|ports| ports.get(&edge.from_port))
            .copied()
            .ok_or_else(|| MaterializeError::MissingOutput {
                node: edge.from_node.clone(),
                port: edge.from_port.clone(),
            })?;
        match kinds.get(edge.to_node.as_str()).copied() {
            Some(GraphNodeKind::Cpu) => cpu_lines.push(CpuLine {
                cpu: edge.to_node.clone(),
                port: edge.to_port.clone(),
                source,
                signal,
            }),
            Some(
                GraphNodeKind::Peripheral | GraphNodeKind::Part | GraphNodeKind::ExternalEndpoint,
            ) => {
                let sink =
                    *nodes
                        .get(&edge.to_node)
                        .ok_or_else(|| MaterializeError::MissingNode {
                            node: edge.to_node.clone(),
                        })?;
                machine.connect(source, signal, sink)?;
            }
            _ => {
                return Err(MaterializeError::UnsupportedSink {
                    edge: edge.id.clone(),
                    node: edge.to_node.clone(),
                });
            }
        }
    }
    cpu_lines.sort_by(|left, right| left.cpu.cmp(&right.cpu).then(left.port.cmp(&right.port)));
    Ok(cpu_lines)
}

fn attach_cpu_views(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
    views: &BTreeMap<String, FlatView>,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    for node in &graph.nodes {
        if node.kind != GraphNodeKind::Cpu {
            continue;
        }
        let view_id = cpu_view(graph, &node.id)?;
        let view = views
            .get(&view_id)
            .cloned()
            .ok_or_else(|| MaterializeError::MissingView {
                cpu: node.id.clone(),
                view: view_id,
            })?;
        machine.add_view(nodes[&node.id], view)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mosaic_contract::mmio::MmioTarget;
    use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
    use mosaic_describe::{GraphEdge, GraphNode, GraphRegion};
    use mosaic_exec::{AccessSize, Attr, BusFault};
    use mosaic_runtime::bus_attr;

    use super::*;

    struct Probe {
        irq: SignalId,
        seen: u64,
    }

    impl MmioTarget for Probe {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            Ok(self.seen)
        }

        fn write(
            &mut self,
            ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            self.seen = value;
            set_optional_signal(ctx, self.irq, value != 0).map_err(|_| BusFault::Device)?;
            Ok(())
        }
    }

    impl EventTarget for Probe {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
            Ok(())
        }

        fn on_signal(
            &mut self,
            _ctx: &mut dyn Ctx,
            _source: NodeId,
            _signal: SignalId,
            high: bool,
        ) -> Result<(), ContextError> {
            self.seen = u64::from(high);
            Ok(())
        }
    }

    fn edge(
        id: &str,
        from: &str,
        from_port: &str,
        to: &str,
        to_port: &str,
        kind: &str,
    ) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            from_node: from.into(),
            from_port: from_port.into(),
            to_node: to.into(),
            to_port: to_port.into(),
            kind: kind.into(),
            parameters: BTreeMap::new(),
        }
    }

    fn slice_graph() -> MachineGraph {
        MachineGraph {
            soc_id: "soc:test/slice".into(),
            board_id: None,
            nodes: vec![
                GraphNode {
                    id: "memory.ram".into(),
                    kind: GraphNodeKind::MemoryBacking,
                    catalog_id: None,
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "memory.flash".into(),
                    kind: GraphNodeKind::MemoryBacking,
                    catalog_id: None,
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "soc.core0".into(),
                    kind: GraphNodeKind::Cpu,
                    catalog_id: Some("cpu:arm/cortex-m3".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "soc.uart".into(),
                    kind: GraphNodeKind::Peripheral,
                    catalog_id: Some("ip:test/uart".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "soc.vic".into(),
                    kind: GraphNodeKind::Peripheral,
                    catalog_id: Some("ip:test/vic".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "view.sysbus".into(),
                    kind: GraphNodeKind::AddressSpaceView,
                    catalog_id: None,
                    parameters: BTreeMap::new(),
                },
            ],
            regions: vec![
                GraphRegion {
                    name: "flash-alias".into(),
                    view: "view.sysbus".into(),
                    target_node: "memory.flash".into(),
                    base: 0,
                    size: 0x1000,
                    kind: RegionKind::Rom,
                    priority: None,
                },
                GraphRegion {
                    name: "flash".into(),
                    view: "view.sysbus".into(),
                    target_node: "memory.flash".into(),
                    base: 0x8000,
                    size: 0x1000,
                    kind: RegionKind::Rom,
                    priority: None,
                },
                GraphRegion {
                    name: "ram".into(),
                    view: "view.sysbus".into(),
                    target_node: "memory.ram".into(),
                    base: 0x2000,
                    size: 0x1000,
                    kind: RegionKind::Ram,
                    priority: None,
                },
                GraphRegion {
                    name: "uart".into(),
                    view: "view.sysbus".into(),
                    target_node: "soc.uart".into(),
                    base: 0x4000,
                    size: 0x100,
                    kind: RegionKind::Mmio,
                    priority: None,
                },
                GraphRegion {
                    name: "vic".into(),
                    view: "view.sysbus".into(),
                    target_node: "soc.vic".into(),
                    base: 0x5000,
                    size: 0x100,
                    kind: RegionKind::Mmio,
                    priority: None,
                },
            ],
            edges: vec![
                edge(
                    "view.core0.space",
                    "soc.core0",
                    "address-space",
                    "view.sysbus",
                    "",
                    "address-space",
                ),
                edge(
                    "irq.uart.combined.0",
                    "soc.uart",
                    "combined",
                    "soc.vic",
                    "inputs:3",
                    "irq",
                ),
                edge("irq.uart.rx.0", "soc.uart", "rx", "soc.core0", "irq", "irq"),
                edge(
                    "clock.uart",
                    "clock.uartclk",
                    "out",
                    "soc.uart",
                    "uartclk",
                    "clock",
                ),
            ],
        }
    }

    fn registry(bound: Arc<Mutex<Option<usize>>>) -> DeviceRegistry {
        let mut registry = DeviceRegistry::new();
        registry
            .register(
                "ip:test/uart",
                move |ctx: &InstantiateCtx, machine: &mut Machine| {
                    let irq = ctx.output("combined")?;
                    machine.register_device(ctx.node, Box::new(Probe { irq, seen: 0 }))?;
                    Ok(())
                },
            )
            .unwrap();
        registry
            .register(
                "ip:test/vic",
                move |ctx: &InstantiateCtx, machine: &mut Machine| {
                    let index = ctx.incoming.iter().find_map(IncomingWire::index);
                    *bound.lock().expect("bound") = index;
                    machine.register_device(
                        ctx.node,
                        Box::new(Probe {
                            irq: SignalId(0),
                            seen: 0,
                        }),
                    )?;
                    Ok(())
                },
            )
            .unwrap();
        registry
    }

    #[test]
    fn declared_irq_reaches_the_sink_and_the_alias_shares_one_backing() {
        let bound = Arc::new(Mutex::new(None));
        let mut assembled = materialize(&slice_graph(), &registry(Arc::clone(&bound)), 8).unwrap();
        assert_eq!(*bound.lock().unwrap(), Some(3));
        assert_eq!(assembled.nodes["memory.flash"], NodeId(0));
        assert_eq!(assembled.nodes["soc.uart"], NodeId(3));
        assert_eq!(assembled.nodes["soc.vic"], NodeId(4));
        assert_eq!(assembled.next_node, 5);
        // The alias and the physical flash window share one backing block.
        let flash = assembled.nodes["memory.flash"];
        assembled
            .machine
            .loader_write(flash, 0, &[0x11, 0x22, 0x33, 0x44])
            .unwrap();
        let attr = bus_attr(assembled.nodes["soc.core0"]);
        let alias = assembled
            .machine
            .bus_read(Addr(0), AccessSize::Word, &attr)
            .unwrap();
        let physical = assembled
            .machine
            .bus_read(Addr(0x8000), AccessSize::Word, &attr)
            .unwrap();
        assert_eq!(alias, 0x4433_2211);
        assert_eq!(physical, alias);
        let rx = assembled.outputs[&("soc.uart".into(), "rx".into())];
        assert_eq!(
            assembled.cpu_lines,
            vec![CpuLine {
                cpu: "soc.core0".into(),
                port: "irq".into(),
                source: assembled.nodes["soc.uart"],
                signal: rx,
            }]
        );
        assert_eq!(assembled.deferred.len(), 1);
        assert_eq!(assembled.deferred[0].kind, "clock");
    }

    #[test]
    fn a_raised_output_is_delivered_to_the_declared_input() {
        let bound = Arc::new(Mutex::new(None));
        let mut assembled = materialize(&slice_graph(), &registry(bound), 8).unwrap();
        let attr = bus_attr(assembled.nodes["soc.core0"]);
        assembled
            .machine
            .bus_write(Addr(0x4000), AccessSize::Word, 1, &attr)
            .unwrap();
        let now = assembled.machine.now();
        assert_eq!(assembled.machine.run_until(now, 8).unwrap(), 1);
        assert_eq!(
            assembled
                .machine
                .bus_read(Addr(0x5000), AccessSize::Word, &attr)
                .unwrap(),
            1
        );
        assert!(assembled
            .machine
            .bus_write(Addr(0x8000), AccessSize::Word, 1, &attr)
            .is_err());
    }

    #[test]
    fn a_missing_factory_is_not_replaced_with_a_stub() {
        let registry = DeviceRegistry::new();
        let error = materialize(&slice_graph(), &registry, 4).unwrap_err();
        assert!(matches!(error, MaterializeError::MissingFactory { .. }));
    }
}
