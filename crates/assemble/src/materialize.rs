//! Turn a normalized [`MachineGraph`] into a runtime [`Machine`].
//!
//! This crate does not know catalog types. The composition root registers a
//! [`DeviceFactory`] per catalog ID (ADR 0016). IRQ fan-out is rejected rather
//! than silently dropped: the runtime wiring table is one sink per output.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mosaic_describe::{GraphEdge, GraphNodeKind, MachineGraph, RegionKind};
use mosaic_exec::{Addr, Mapping, MemoryKind, NodeId, Permissions, SignalId};
use mosaic_runtime::{AddressError, FlatView, Machine, MachineError, Region};

use crate::faces::{connect_faces, FabricStore};

/// Why a declared machine could not be instantiated.
#[derive(Debug)]
pub enum MaterializeError {
    DuplicateFactory {
        catalog_id: String,
    },
    MissingFactory {
        catalog_id: String,
        node: String,
    },
    MissingNode {
        node: String,
    },
    MissingView {
        cpu: String,
        view: String,
    },
    MissingOutput {
        node: String,
        port: String,
    },
    Fanout {
        node: String,
        port: String,
    },
    UnsupportedSink {
        edge: String,
        node: String,
    },
    MultipleViews,
    Address(AddressError),
    Machine(MachineError),
    Bind {
        node: String,
        detail: String,
    },
    /// An i2c/spi edge references an initiator face whose spawn never
    /// claimed or offered a segment bus (ADR 0032).
    MissingSegment {
        node: String,
        port: String,
    },
    /// An i2c/spi edge references a target face the part never exposed.
    MissingTargetFace {
        node: String,
        port: String,
    },
    /// A gpio edge references a line the part never exposed.
    MissingGpioLine {
        node: String,
        port: String,
        index: u64,
    },
    /// A face joined its fabric but the fabric rejected the attachment.
    FaceAttach {
        edge: String,
        detail: String,
    },
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
            Self::MissingSegment { node, port } => write!(
                f,
                "node {node:?} masters no fabric segment on face {port:?}; its spawn must \
                 claim or offer one before parts can attach (ADR 0032)"
            ),
            Self::MissingTargetFace { node, port } => write!(
                f,
                "node {node:?} never exposed the target face {port:?} an edge wires onto"
            ),
            Self::MissingGpioLine { node, port, index } => {
                write!(f, "node {node:?} never exposed gpio line {port:?}[{index}]")
            }
            Self::FaceAttach { edge, detail } => write!(f, "face edge {edge:?}: {detail}"),
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
    /// The producing node's output port name (`gate-apb2-iopa`, `combined`).
    pub source_port: String,
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
pub struct InstantiateCtx<'a> {
    pub graph_id: &'a str,
    pub catalog_id: &'a str,
    pub node: NodeId,
    pub nodes: &'a BTreeMap<String, NodeId>,
    pub outputs: &'a BTreeMap<String, SignalId>,
    pub incoming: &'a [IncomingWire],
    /// The node's manifest parameters (graph node `parameters` map).
    pub parameters: &'a BTreeMap<String, mosaic_describe::ConfigValue>,
    /// The machine harness's host-side endpoints, by node name.
    pub hosts: &'a HostAttachments,
    /// Fabric faces this node *receives* (i2c/spi target faces wired onto
    /// it, with their wiring parameters — ADR 0032).
    pub incoming_faces: &'a [&'a GraphEdge],
    /// Fabric faces this node *masters* (i2c/spi edges leaving it).
    pub outgoing_faces: &'a [&'a GraphEdge],
    /// The fabric faces every spawn shares (segments, nets, exposures).
    /// The factory signature passes a shared context, so the store sits
    /// behind a mutex the face methods lock per call.
    pub fabrics: &'a std::sync::Mutex<FabricStore>,
}

impl std::fmt::Debug for InstantiateCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstantiateCtx")
            .field("graph_id", &self.graph_id)
            .field("catalog_id", &self.catalog_id)
            .field("node", &self.node)
            .field("incoming", &self.incoming.len())
            .field("incoming_faces", &self.incoming_faces.len())
            .field("outgoing_faces", &self.outgoing_faces.len())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for HostAttachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostAttachments")
            .field("uart", &self.uart.len())
            .field("i2c", &self.i2c.len())
            .field("spi", &self.spi.len())
            .field("usb", &self.usb.len())
            .field("net", &self.net.len())
            .field("sd", &self.sd.len())
            .finish()
    }
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

    /// A string manifest parameter of this node, when present.
    pub fn parameter_str(&self, name: &str) -> Option<&str> {
        self.parameters.get(name).and_then(|value| value.as_str())
    }

    /// An integer manifest parameter of this node, when present.
    pub fn parameter_int(&self, name: &str) -> Option<i64> {
        self.parameters
            .get(name)
            .and_then(|value| value.as_integer())
    }

    /// The host UART endpoint attached to this node, if the harness provided
    /// one (ADR 0025: host I/O is the machine's, models only ask).
    pub fn host_uart(&self) -> Option<&mosaic_bus::uart::pair::UartEndpoint> {
        self.hosts.uart(self.graph_id)
    }

    /// The host I2C fabric attached to this node (its targets are the
    /// board's wiring).
    pub fn host_i2c(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::i2c::core::I2cBus>>> {
        self.hosts.i2c(self.graph_id)
    }

    /// The host SPI fabric attached to this node.
    pub fn host_spi(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::spi::core::SpiBus>>> {
        self.hosts.spi(self.graph_id)
    }

    /// The host USB fabric attached to this node.
    pub fn host_usb(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::usb::core::UsbBus>>> {
        self.hosts.usb(self.graph_id)
    }

    /// The host Ethernet link attached to this node.
    pub fn host_net(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::net::link::EthernetLink>>> {
        self.hosts.net(self.graph_id)
    }

    /// The host SD/eMMC card attached to this node.
    pub fn host_sd(&self) -> Option<std::sync::Arc<mosaic_bus::sd::SdCardHandle>> {
        self.hosts.sd(&self.local_name())
    }
    /// The guest-memory (bus-master) port attached to this node, when the
    /// harness provided one (virtio-class devices, ADR 0033).
    pub fn host_guest_memory(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<dyn mosaic_contract::guest_mem::GuestMemory>>> {
        self.hosts.guest_memory(&self.local_name())
    }

    /// The block image attached to this node, when the harness provided
    /// one (storage-class devices, ADR 0033).
    pub fn host_block(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<Box<dyn mosaic_contract::block::BlockImage>>>> {
        self.hosts.block(&self.local_name())
    }

    /// This node's manifest name (the reverse of the nodes table).
    fn local_name(&self) -> String {
        self.nodes
            .iter()
            .find(|(_, node)| **node == self.node)
            .map(|(name, _)| name.clone())
            .unwrap_or_default()
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

    /// The vendor SignalId one of this model's output ports drives (ADR
    /// 0025): clock-gate tables such as the RCC's `gate-apb2-iopa` publish
    /// model-internal signal numbers here, so a `clock` edge can resolve
    /// without a board wiring it by hand. The default reports none.
    fn output_signal(&self, _port: &str) -> Option<SignalId> {
        None
    }
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

/// Host-side attachments the machine harness provides (ADR 0025): the UART
/// console endpoint a host shows, keyed by the graph node that owns the
/// guest-side UART. A factory asks through [`InstantiateCtx::host_uart`]; a
/// model without a host attachment simply runs without one.
#[derive(Default)]
#[allow(clippy::missing_fields_in_debug)]
pub struct HostAttachments {
    uart: BTreeMap<String, mosaic_bus::uart::pair::UartEndpoint>,
    i2c: BTreeMap<String, std::sync::Arc<std::sync::Mutex<mosaic_bus::i2c::core::I2cBus>>>,
    spi: BTreeMap<String, std::sync::Arc<std::sync::Mutex<mosaic_bus::spi::core::SpiBus>>>,
    usb: BTreeMap<String, std::sync::Arc<std::sync::Mutex<mosaic_bus::usb::core::UsbBus>>>,
    net: BTreeMap<String, std::sync::Arc<std::sync::Mutex<mosaic_bus::net::link::EthernetLink>>>,
    sd: BTreeMap<String, std::sync::Arc<mosaic_bus::sd::SdCardHandle>>,
    guest_memory: BTreeMap<
        String,
        std::sync::Arc<std::sync::Mutex<dyn mosaic_contract::guest_mem::GuestMemory>>,
    >,
    block: BTreeMap<
        String,
        std::sync::Arc<std::sync::Mutex<Box<dyn mosaic_contract::block::BlockImage>>>,
    >,
}

impl HostAttachments {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a host UART endpoint for the graph node named `node`
    /// (`soc.usart1`): the peer end stays with the caller, which prints it.
    pub fn attach_uart(&mut self, node: &str, endpoint: mosaic_bus::uart::pair::UartEndpoint) {
        self.uart.insert(node.to_owned(), endpoint);
    }

    pub fn uart(&self, node: &str) -> Option<&mosaic_bus::uart::pair::UartEndpoint> {
        self.uart.get(node)
    }

    /// Attach an I2C fabric (its targets are already composed — bus
    /// membership is the board's wiring, ADR 0025) for an I2C controller.
    pub fn attach_i2c(
        &mut self,
        node: &str,
        bus: std::sync::Arc<std::sync::Mutex<mosaic_bus::i2c::core::I2cBus>>,
    ) {
        self.i2c.insert(node.to_owned(), bus);
    }

    pub fn i2c(
        &self,
        node: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::i2c::core::I2cBus>>> {
        self.i2c.get(node).cloned()
    }

    /// Attach an SPI fabric (with its targets composed) for an SPI controller.
    pub fn attach_spi(
        &mut self,
        node: &str,
        bus: std::sync::Arc<std::sync::Mutex<mosaic_bus::spi::core::SpiBus>>,
    ) {
        self.spi.insert(node.to_owned(), bus);
    }

    pub fn spi(
        &self,
        node: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::spi::core::SpiBus>>> {
        self.spi.get(node).cloned()
    }

    /// Attach a USB fabric (with its devices enumerated) for a host
    /// controller.
    pub fn attach_usb(
        &mut self,
        node: &str,
        bus: std::sync::Arc<std::sync::Mutex<mosaic_bus::usb::core::UsbBus>>,
    ) {
        self.usb.insert(node.to_owned(), bus);
    }

    pub fn usb(
        &self,
        node: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::usb::core::UsbBus>>> {
        self.usb.get(node).cloned()
    }

    /// Attach an Ethernet link (with its ports composed) for a NIC.
    pub fn attach_net(
        &mut self,
        node: &str,
        link: std::sync::Arc<std::sync::Mutex<mosaic_bus::net::link::EthernetLink>>,
    ) {
        self.net.insert(node.to_owned(), link);
    }

    pub fn net(
        &self,
        node: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<mosaic_bus::net::link::EthernetLink>>> {
        self.net.get(node).cloned()
    }

    /// Attach an SD/eMMC card for an SD host controller.
    pub fn attach_sd(&mut self, node: &str, card: std::sync::Arc<mosaic_bus::sd::SdCardHandle>) {
        self.sd.insert(node.to_owned(), card);
    }

    /// Attach a guest-memory (bus-master) port for a device that walks
    /// guest RAM directly (virtio rings; ADR 0033).
    pub fn attach_guest_memory(
        &mut self,
        node: &str,
        memory: std::sync::Arc<std::sync::Mutex<dyn mosaic_contract::guest_mem::GuestMemory>>,
    ) {
        self.guest_memory.insert(node.to_owned(), memory);
    }

    pub fn guest_memory(
        &self,
        node: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<dyn mosaic_contract::guest_mem::GuestMemory>>> {
        self.guest_memory.get(node).cloned()
    }

    /// Attach a block image for a storage device (virtio-blk, SCSI;
    /// ADR 0033).
    pub fn attach_block(
        &mut self,
        node: &str,
        image: std::sync::Arc<std::sync::Mutex<Box<dyn mosaic_contract::block::BlockImage>>>,
    ) {
        self.block.insert(node.to_owned(), image);
    }

    pub fn block(
        &self,
        node: &str,
    ) -> Option<std::sync::Arc<std::sync::Mutex<Box<dyn mosaic_contract::block::BlockImage>>>> {
        self.block.get(node).cloned()
    }

    pub fn sd(&self, node: &str) -> Option<std::sync::Arc<mosaic_bus::sd::SdCardHandle>> {
        self.sd.get(node).cloned()
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
    /// Fabric faces after the join pass: mux segments, declared nets and
    /// unclaimed exposures (ADR 0032). None keeps the pre-face shape for
    /// callers that have not opted in.
    pub fabrics: Option<FabricStore>,
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
            .field("fabrics", &self.fabrics.is_some())
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
    hosts: &HostAttachments,
    queue_capacity: usize,
) -> Result<AssembledMachine, MaterializeError> {
    let nodes = allocate_nodes(graph)?;
    let assigned = assign_irq_outputs(graph)?;
    let incoming = incoming_wires(graph, &nodes, &assigned, registry)?;
    let views = build_views(graph, &nodes)?;
    let view = default_view(graph, &views)?;
    let mut machine = Machine::new(view.clone(), queue_capacity);
    add_backings(graph, &nodes, &mut machine)?;
    // The factory signature hands spawns a shared context, so the store the
    // face methods mutate lives behind a mutex until every spawn is done.
    let fabric_cells = std::sync::Mutex::new(FabricStore::default());
    instantiate(
        graph,
        registry,
        &nodes,
        &assigned,
        &incoming,
        hosts,
        &mut machine,
        &fabric_cells,
    )?;
    let mut fabrics = fabric_cells
        .into_inner()
        .map_err(|_| MaterializeError::FaceAttach {
            edge: "<materialize>".into(),
            detail: "fabric store mutex poisoned".into(),
        })?;
    let cpu_lines = connect_wires(graph, &nodes, &assigned, registry, &mut machine)?;
    connect_faces(graph, &nodes, &mut fabrics)?;
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
        .filter(|edge| {
            !matches!(
                edge.kind.as_str(),
                "irq" | "clock" | "address-space" | "i2c" | "spi" | "gpio"
            )
        })
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
        fabrics: Some(fabrics),
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
    registry: &DeviceRegistry,
) -> Result<BTreeMap<String, Vec<IncomingWire>>, MaterializeError> {
    let mut incoming: BTreeMap<String, Vec<IncomingWire>> = BTreeMap::new();
    // `irq` edges resolve through the assigned per-node output space (1..N
    // in port-name order); `clock` edges resolve through the source model's
    // vendor-published output table (ADR 0025) — e.g. the RCC's gates.
    for edge in graph
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind.as_str(), "irq" | "clock"))
    {
        // Abstract clock nets (`clock.uartclk`) have no device behind them:
        // the wire reaches the sink as a wired input with the sink as its
        // own source, signal 0 (the net's driver is not a node).
        let source = match nodes.get(&edge.from_node) {
            Some(source) => *source,
            None if edge.kind == "clock" => {
                *nodes
                    .get(&edge.to_node)
                    .ok_or_else(|| MaterializeError::MissingNode {
                        node: edge.to_node.clone(),
                    })?
            }
            None => {
                return Err(MaterializeError::MissingNode {
                    node: edge.from_node.clone(),
                })
            }
        };
        let signal = if edge.kind == "irq" {
            assigned
                .get(&edge.from_node)
                .and_then(|ports| ports.get(&edge.from_port))
                .copied()
                .ok_or_else(|| MaterializeError::MissingOutput {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                })?
        } else if nodes.contains_key(&edge.from_node) {
            let factory = registry
                .factories
                .get(
                    graph
                        .nodes
                        .iter()
                        .find(|node| node.id == edge.from_node)
                        .and_then(|node| node.catalog_id.as_deref())
                        .unwrap_or_default(),
                )
                .ok_or_else(|| MaterializeError::MissingFactory {
                    catalog_id: edge.from_node.clone(),
                    node: edge.from_node.clone(),
                })?;
            factory.output_signal(&edge.from_port).ok_or_else(|| {
                MaterializeError::MissingOutput {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                }
            })?
        } else {
            SignalId(0)
        };
        incoming
            .entry(edge.to_node.clone())
            .or_default()
            .push(IncomingWire {
                port: edge.to_port.clone(),
                source_port: edge.from_port.clone(),
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

#[allow(clippy::too_many_arguments)]
fn instantiate(
    graph: &MachineGraph,
    registry: &DeviceRegistry,
    nodes: &BTreeMap<String, NodeId>,
    assigned: &BTreeMap<String, BTreeMap<String, SignalId>>,
    incoming: &BTreeMap<String, Vec<IncomingWire>>,
    hosts: &HostAttachments,
    machine: &mut Machine,
    fabrics: &std::sync::Mutex<FabricStore>,
) -> Result<(), MaterializeError> {
    // Face slices per node: target faces arrive (with wiring parameters),
    // initiator faces leave (a controller checks whether any target rides
    // it before preferring its host attachment).
    let face_edges: Vec<&GraphEdge> = graph
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind.as_str(), "i2c" | "spi" | "gpio"))
        .collect();
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
        let incoming_faces: Vec<&GraphEdge> = face_edges
            .iter()
            .copied()
            .filter(|edge| edge.to_node == node.id)
            .collect();
        let outgoing_faces: Vec<&GraphEdge> = face_edges
            .iter()
            .copied()
            .filter(|edge| edge.from_node == node.id)
            .collect();
        let ctx = InstantiateCtx {
            graph_id: &node.id,
            catalog_id,
            node: nodes[&node.id],
            nodes,
            outputs: assigned.get(&node.id).unwrap_or(&empty_outputs),
            incoming: incoming.get(&node.id).unwrap_or(&empty_incoming),
            parameters: &node.parameters,
            hosts,
            incoming_faces: &incoming_faces,
            outgoing_faces: &outgoing_faces,
            fabrics,
        };
        factory.instantiate(&ctx, machine)?;
    }
    Ok(())
}

fn connect_wires(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
    assigned: &BTreeMap<String, BTreeMap<String, SignalId>>,
    registry: &DeviceRegistry,
    machine: &mut Machine,
) -> Result<Vec<CpuLine>, MaterializeError> {
    let kinds: BTreeMap<&str, GraphNodeKind> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind))
        .collect();
    let mut cpu_lines = Vec::new();
    for edge in graph
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind.as_str(), "irq" | "clock"))
    {
        // Abstract clock nets have no device to connect from: the wire is
        // delivered to the sink factory as an incoming wire only.
        if edge.kind == "clock" && !nodes.contains_key(&edge.from_node) {
            continue;
        }
        let source = *nodes
            .get(&edge.from_node)
            .ok_or_else(|| MaterializeError::MissingNode {
                node: edge.from_node.clone(),
            })?;
        let signal = if edge.kind == "irq" {
            assigned
                .get(&edge.from_node)
                .and_then(|ports| ports.get(&edge.from_port))
                .copied()
                .ok_or_else(|| MaterializeError::MissingOutput {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                })?
        } else {
            let factory = registry
                .factories
                .get(
                    graph
                        .nodes
                        .iter()
                        .find(|node| node.id == edge.from_node)
                        .and_then(|node| node.catalog_id.as_deref())
                        .unwrap_or_default(),
                )
                .ok_or_else(|| MaterializeError::MissingFactory {
                    catalog_id: edge.from_node.clone(),
                    node: edge.from_node.clone(),
                })?;
            factory.output_signal(&edge.from_port).ok_or_else(|| {
                MaterializeError::MissingOutput {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                }
            })?
        };
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
                // Parts have no arena presence (their faces carry them),
                // so their outputs wire through the part-source path.
                if kinds.get(edge.from_node.as_str()).copied() == Some(GraphNodeKind::Part) {
                    machine.connect_output(source, signal, sink)?;
                } else {
                    machine.connect(source, signal, sink)?;
                }
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
        let mut assembled = materialize(
            &slice_graph(),
            &registry(Arc::clone(&bound)),
            &HostAttachments::new(),
            8,
        )
        .unwrap();
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
        // Clock edges are wired now (ADR 0025): the abstract net's wire
        // reached the sink factory as an incoming wire, nothing deferred.
        assert!(assembled.deferred.is_empty());
    }

    #[test]
    fn a_raised_output_is_delivered_to_the_declared_input() {
        let bound = Arc::new(Mutex::new(None));
        let mut assembled =
            materialize(&slice_graph(), &registry(bound), &HostAttachments::new(), 8).unwrap();
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
        let error = materialize(&slice_graph(), &registry, &HostAttachments::new(), 4).unwrap_err();
        assert!(matches!(error, MaterializeError::MissingFactory { .. }));
    }
}
