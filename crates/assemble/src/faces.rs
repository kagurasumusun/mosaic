//! The face-connection pass: wiring multi-face models onto fabrics (ADR 0032).
//!
//! `materialize` instantiates every node first — each spawn owns, offers or
//! exposes its port faces through the [`FabricStore`] — and this module then
//! joins the declared `i2c` / `spi` / `gpio` edges:
//!
//! * an **i2c/spi edge** resolves the initiator's segment bus (a controller
//!   claimed it with [`InstantiateCtx::i2c_segment`] or a mux offered it with
//!   [`InstantiateCtx::offer_i2c_segment`]) and attaches the target face the
//!   part exposed, so one part joins a bus without either fabric knowing the
//!   model behind the other end;
//! * a **gpio edge** resolves (or creates) the named net, attaches a terminal
//!   with the declared pull and hands the terminal to the part's line face.
//!
//! Nothing here is deferred: a face edge that cannot be joined is a
//! materialization error, not a silent no-op.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mosaic_bus::gpio::core::GpioNet;
use mosaic_bus::i2c::core::I2cBus;
use mosaic_bus::spi::core::SpiBus;
use mosaic_contract::i2c::I2cTarget;
use mosaic_contract::net::{DigitalNet, NetNode, NetPull};
use mosaic_contract::spi::SpiTarget;
use mosaic_describe::{ConfigValue, GraphEdge, MachineGraph};
use mosaic_exec::NodeId;

use crate::materialize::{InstantiateCtx, MaterializeError};

/// One exposed GPIO line of a part, waiting for its net terminal.
///
/// The part keeps its own line semantics (latches, direction registers,
/// interrupt-on-change) and receives the electrical terminal once the
/// declared net exists. Implementations register the observer side with
/// `GpioNet::attach_sink` and keep the drive side `GpioNetPort` from
/// `GpioNet::port`.
pub trait GpioLineJoin: Send {
    /// Join this line to its net terminal `node` on `net`.
    fn join(&mut self, net: &mut GpioNet, node: NetNode);
}

/// Everything the spawns left for the face pass: initiator segments
/// (controller- and mux-owned), exposed target faces and GPIO lines, and
/// the nets the board declared by name.
#[derive(Default)]
pub struct FabricStore {
    i2c_segments: BTreeMap<(u32, String), Arc<Mutex<I2cBus>>>,
    spi_segments: BTreeMap<(u32, String), Arc<Mutex<SpiBus>>>,
    i2c_targets: BTreeMap<(u32, String), Box<dyn I2cTarget>>,
    spi_targets: BTreeMap<(u32, String), (u8, Box<dyn SpiTarget>)>,
    gpio_lines: BTreeMap<(u32, String, u64), Box<dyn GpioLineJoin>>,
    gpio_nets: BTreeMap<String, GpioNet>,
}

impl FabricStore {
    /// The segment bus an initiator face owns, if its spawn claimed or
    /// offered one. Boards and tests use this to reach a mux's downstream
    /// segments after assembly.
    pub fn i2c_segment(&self, node: NodeId, port: &str) -> Option<Arc<Mutex<I2cBus>>> {
        self.i2c_segments.get(&(node.0, port.to_owned())).cloned()
    }

    /// The net a `wire:` name resolved to, if any gpio face joined it.
    /// `GpioNet` is Arc-backed, so the clone shares the net's state: boards
    /// and tests attach their drivers and observers through it.
    pub fn gpio_net(&self, wire: &str) -> Option<GpioNet> {
        self.gpio_nets.get(wire).cloned()
    }

    fn claim_i2c_segment(&mut self, node: NodeId, port: &str) -> Arc<Mutex<I2cBus>> {
        self.i2c_segments
            .entry((node.0, port.to_owned()))
            .or_insert_with(|| Arc::new(Mutex::new(I2cBus::new())))
            .clone()
    }

    fn claim_spi_segment(&mut self, node: NodeId, port: &str) -> Arc<Mutex<SpiBus>> {
        self.spi_segments
            .entry((node.0, port.to_owned()))
            .or_insert_with(|| Arc::new(Mutex::new(SpiBus::new())))
            .clone()
    }
}

impl InstantiateCtx<'_> {
    /// Whether an i2c target face is wired onto this node's initiator face
    /// `port`. Controllers with a host-attached fabric keep using it when no
    /// face wiring exists (the pc glue path); when faces are wired the
    /// face-owned segment is the fabric instead.
    pub fn i2c_face_wired(&self, port: &str) -> bool {
        self.outgoing_faces
            .iter()
            .any(|edge| edge.kind == "i2c" && edge.from_port == port)
    }

    /// The segment bus this node masters on initiator face `port`, created
    /// on first use. Target faces wired onto `(this node, port)` attach to
    /// exactly this bus during the face pass.
    pub fn i2c_segment(&self, port: &str) -> Arc<Mutex<I2cBus>> {
        self.fabrics
            .lock()
            .expect("fabric store poisoned")
            .claim_i2c_segment(self.node, port)
    }

    /// The SPI twin of [`InstantiateCtx::i2c_segment`].
    pub fn spi_segment(&self, port: &str) -> Arc<Mutex<SpiBus>> {
        self.fabrics
            .lock()
            .expect("fabric store poisoned")
            .claim_spi_segment(self.node, port)
    }

    /// Register a segment bus this node brought with it — a mux's downstream
    /// channels own their buses from construction. Faces wired `to` this
    /// node's `port` attach to the offered bus.
    pub fn offer_i2c_segment(&self, port: &str, bus: Arc<Mutex<I2cBus>>) {
        self.fabrics
            .lock()
            .expect("fabric store poisoned")
            .offer_i2c_segment(self.node, port, bus);
    }

    /// Expose a target face: the face pass attaches it onto the initiator
    /// referenced by the `[[part.face]]` wiring (with the address the spawn
    /// read through [`InstantiateCtx::face_params`]).
    pub fn expose_i2c_target(&self, port: &str, target: Box<dyn I2cTarget>) {
        self.fabrics
            .lock()
            .expect("fabric store poisoned")
            .expose_i2c_target(self.node, port, target);
    }

    /// Expose an SPI target face at chip-select `cs`.
    pub fn expose_spi_target(&self, port: &str, cs: u8, target: Box<dyn SpiTarget>) {
        self.fabrics
            .lock()
            .expect("fabric store poisoned")
            .expose_spi_target(self.node, port, cs, target);
    }

    /// Expose GPIO line `index` of face `port`; the face pass joins it to
    /// the net the board declared for that line.
    pub fn expose_gpio_line(&self, port: &str, index: u64, line: Box<dyn GpioLineJoin>) {
        self.fabrics
            .lock()
            .expect("fabric store poisoned")
            .expose_gpio_line(self.node, port, index, line);
    }

    /// The wiring parameters of the target face `port` wired onto this node
    /// (for example the I2C `address`), or none when the board left that
    /// face unwired.
    pub fn face_params(&self, port: &str) -> Option<&BTreeMap<String, ConfigValue>> {
        self.incoming_faces
            .iter()
            .find(|edge| edge.to_port == port)
            .map(|edge| &edge.parameters)
    }
}

// Interior-mutability plumbing: `DeviceFactory::instantiate` hands spawns an
// shared `&InstantiateCtx`, so the shared store lives behind a mutex the
// spawn methods lock per call. Assembly is single-threaded; the mutex only
// satisfies the borrow shapes, like the engine bus's own machine mutex.
impl FabricStore {
    fn offer_i2c_segment(&mut self, node: NodeId, port: &str, bus: Arc<Mutex<I2cBus>>) {
        self.i2c_segments.insert((node.0, port.to_owned()), bus);
    }

    fn expose_i2c_target(&mut self, node: NodeId, port: &str, target: Box<dyn I2cTarget>) {
        self.i2c_targets.insert((node.0, port.to_owned()), target);
    }

    fn expose_spi_target(&mut self, node: NodeId, port: &str, cs: u8, target: Box<dyn SpiTarget>) {
        self.spi_targets
            .insert((node.0, port.to_owned()), (cs, target));
    }

    fn expose_gpio_line(
        &mut self,
        node: NodeId,
        port: &str,
        index: u64,
        line: Box<dyn GpioLineJoin>,
    ) {
        self.gpio_lines
            .insert((node.0, port.to_owned(), index), line);
    }
}

/// Join every declared fabric face. Runs after all spawns, so controllers
/// and muxes have claimed their segments and parts have exposed their
/// target and line faces.
pub(crate) fn connect_faces(
    graph: &MachineGraph,
    nodes: &BTreeMap<String, NodeId>,
    fabrics: &mut FabricStore,
) -> Result<(), MaterializeError> {
    for edge in graph
        .edges
        .iter()
        .filter(|edge| matches!(edge.kind.as_str(), "i2c" | "spi" | "gpio"))
    {
        join_edge(edge, nodes, fabrics)?;
    }
    Ok(())
}

fn join_edge(
    edge: &GraphEdge,
    nodes: &BTreeMap<String, NodeId>,
    fabrics: &mut FabricStore,
) -> Result<(), MaterializeError> {
    let from =
        nodes
            .get(&edge.from_node)
            .copied()
            .ok_or_else(|| MaterializeError::MissingNode {
                node: edge.from_node.clone(),
            })?;
    let to = nodes
        .get(&edge.to_node)
        .copied()
        .ok_or_else(|| MaterializeError::MissingNode {
            node: edge.to_node.clone(),
        })?;
    match edge.kind.as_str() {
        "i2c" => {
            let Some(bus) = fabrics
                .i2c_segments
                .get(&(from.0, edge.from_port.clone()))
                .cloned()
            else {
                return Err(MaterializeError::MissingSegment {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                });
            };
            let Some(target) = fabrics.i2c_targets.remove(&(to.0, edge.to_port.clone())) else {
                return Err(MaterializeError::MissingTargetFace {
                    node: edge.to_node.clone(),
                    port: edge.to_port.clone(),
                });
            };
            bus.lock()
                .map_err(|_| MaterializeError::FaceAttach {
                    edge: edge.id.clone(),
                    detail: "i2c segment mutex poisoned".into(),
                })?
                .add_target(target);
        }
        "spi" => {
            let Some(bus) = fabrics
                .spi_segments
                .get(&(from.0, edge.from_port.clone()))
                .cloned()
            else {
                return Err(MaterializeError::MissingSegment {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                });
            };
            let Some((cs, target)) = fabrics.spi_targets.remove(&(to.0, edge.to_port.clone()))
            else {
                return Err(MaterializeError::MissingTargetFace {
                    node: edge.to_node.clone(),
                    port: edge.to_port.clone(),
                });
            };
            bus.lock()
                .map_err(|_| MaterializeError::FaceAttach {
                    edge: edge.id.clone(),
                    detail: "spi segment mutex poisoned".into(),
                })?
                .add_target(mosaic_contract::spi::SpiCs(cs), target);
        }
        "gpio" => {
            let index = edge
                .parameters
                .get("index")
                .and_then(ConfigValue::as_integer)
                .unwrap_or_default();
            let index = u64::try_from(index).unwrap_or_default();
            let Some(mut line) =
                fabrics
                    .gpio_lines
                    .remove(&(from.0, edge.from_port.clone(), index))
            else {
                return Err(MaterializeError::MissingGpioLine {
                    node: edge.from_node.clone(),
                    port: edge.from_port.clone(),
                    index,
                });
            };
            let net = fabrics.gpio_nets.entry(edge.to_node.clone()).or_default();
            let node = net.attach(pull_of(edge));
            line.join(net, node);
        }
        _ => unreachable!("filtered by connect_faces"),
    }
    Ok(())
}

/// The board's pull declaration for a gpio face: `pull = 'up' | 'down'`
/// (anything else attaches unpulled).
fn pull_of(edge: &GraphEdge) -> NetPull {
    match edge
        .parameters
        .get("pull")
        .and_then(ConfigValue::as_str)
        .unwrap_or_default()
    {
        "up" => NetPull::Up,
        "down" => NetPull::Down,
        _ => NetPull::None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mosaic_bus::i2c::core::I2cBus;
    use mosaic_contract::gpio::{GpioSink, PinLevel};
    use mosaic_contract::i2c::{Ack, I2cAddr, I2cError, I2cInitiator, I2cTarget};
    use mosaic_contract::model::{ContextError, Ctx};
    use mosaic_contract::spi::{SpiError, SpiTarget};
    use mosaic_describe::{GraphEdge, GraphNode, GraphNodeKind, MachineGraph};
    use mosaic_exec::{EventId, SignalId, VTime};

    use super::*;
    use crate::materialize::materialize;
    use crate::{DeviceRegistry, HostAttachments};

    /// A minimal Ctx for fabric-level calls inside tests.
    struct NullCtx;
    impl Ctx for NullCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }
        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }
        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }

    /// A target face that records writes and serves 0x5A reads.
    struct Recorder {
        seen: Arc<Mutex<Vec<u8>>>,
    }

    impl I2cTarget for Recorder {
        fn matches(&self, _addr: &I2cAddr) -> bool {
            true
        }
        fn target_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            bytes: &[u8],
        ) -> Result<Ack, I2cError> {
            self.seen.lock().unwrap().extend_from_slice(bytes);
            Ok(Ack::Ack)
        }
        fn target_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            buf: &mut [u8],
        ) -> Result<Ack, I2cError> {
            buf.fill(0x5A);
            Ok(Ack::Ack)
        }
    }

    /// A controller spawn: claims its `i2c` initiator face unconditionally.
    fn spawn_controller(
        ctx: &InstantiateCtx,
        _machine: &mut mosaic_runtime::Machine,
    ) -> Result<(), MaterializeError> {
        let _bus = ctx.i2c_segment("i2c");
        Ok(())
    }

    /// A part spawn: reads its wired address from the face parameters and
    /// exposes its target face plus one gpio line.
    fn spawn_sensor(
        ctx: &InstantiateCtx,
        _machine: &mut mosaic_runtime::Machine,
    ) -> Result<(), MaterializeError> {
        let address = ctx
            .face_params("i2c")
            .and_then(|params| params.get("address"))
            .and_then(ConfigValue::as_integer)
            .unwrap_or(0x20) as u8;
        let addr = I2cAddr::seven(address).map_err(|_| MaterializeError::Bind {
            node: ctx.graph_id.to_owned(),
            detail: "bad face address".into(),
        })?;
        let shared = Arc::new(Mutex::new(I2cBus::new()));
        let _ = shared;
        ctx.expose_i2c_target(
            "i2c",
            Box::new(AddressFilter {
                addr,
                inner: Recorder {
                    seen: Arc::new(Mutex::new(Vec::new())),
                },
            }),
        );
        ctx.expose_gpio_line(
            "gpio",
            0,
            Box::new(LineProbe {
                joined: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        Ok(())
    }

    /// Wraps the recorder with the strap-selected address from the face.
    struct AddressFilter {
        addr: I2cAddr,
        inner: Recorder,
    }

    impl I2cTarget for AddressFilter {
        fn matches(&self, addr: &I2cAddr) -> bool {
            addr == &self.addr
        }
        fn target_write(
            &mut self,
            ctx: &mut dyn Ctx,
            addr: &I2cAddr,
            bytes: &[u8],
        ) -> Result<Ack, I2cError> {
            self.inner.target_write(ctx, addr, bytes)
        }
        fn target_read(
            &mut self,
            ctx: &mut dyn Ctx,
            addr: &I2cAddr,
            buf: &mut [u8],
        ) -> Result<Ack, I2cError> {
            self.inner.target_read(ctx, addr, buf)
        }
    }

    /// A gpio line face that observes the joined net.
    struct LineProbe {
        joined: Arc<Mutex<Vec<PinLevel>>>,
    }

    struct ProbeSink(Arc<Mutex<Vec<PinLevel>>>);
    impl GpioSink for ProbeSink {
        fn level_changed(&mut self, _ctx: &mut dyn Ctx, level: PinLevel) {
            self.0.lock().unwrap().push(level);
        }
    }

    impl GpioLineJoin for LineProbe {
        fn join(&mut self, net: &mut GpioNet, node: NetNode) {
            net.attach_sink(node, Box::new(ProbeSink(Arc::clone(&self.joined))))
                .expect("attach probe sink");
        }
    }

    fn face_graph() -> MachineGraph {
        MachineGraph {
            soc_id: "soc:test/slice".into(),
            board_id: Some("board:test".into()),
            nodes: vec![
                GraphNode {
                    id: "soc.ctrl".into(),
                    kind: GraphNodeKind::Peripheral,
                    catalog_id: Some("ip:test/ctrl".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "board.sensor".into(),
                    kind: GraphNodeKind::Part,
                    catalog_id: Some("part:test/sensor".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "wire:sensor-line".into(),
                    kind: GraphNodeKind::ExternalEndpoint,
                    catalog_id: None,
                    parameters: BTreeMap::new(),
                },
            ],
            regions: Vec::new(),
            edges: vec![
                GraphEdge {
                    id: "face.sensor.i2c".into(),
                    from_node: "soc.ctrl".into(),
                    from_port: "i2c".into(),
                    to_node: "board.sensor".into(),
                    to_port: "i2c".into(),
                    kind: "i2c".into(),
                    parameters: [("address".to_owned(), ConfigValue::Integer(0x20))]
                        .into_iter()
                        .collect(),
                },
                GraphEdge {
                    id: "face.sensor.gpio.0".into(),
                    from_node: "board.sensor".into(),
                    from_port: "gpio".into(),
                    to_node: "wire:sensor-line".into(),
                    to_port: String::new(),
                    kind: "gpio".into(),
                    parameters: [("index".to_owned(), ConfigValue::Integer(0))]
                        .into_iter()
                        .collect(),
                },
            ],
        }
    }

    #[test]
    fn face_edges_join_targets_and_lines_onto_fabrics() {
        let graph = face_graph();
        let mut registry = DeviceRegistry::new();
        registry.register("ip:test/ctrl", spawn_controller).unwrap();
        registry.register("part:test/sensor", spawn_sensor).unwrap();
        let assembled = materialize(&graph, &registry, &HostAttachments::new(), 8).unwrap();
        // The controller's claimed segment exists and carries the part's
        // target face: a transaction at the wired address reaches it.
        let ctrl = assembled.nodes["soc.ctrl"];
        let segment = assembled
            .fabrics
            .as_ref()
            .expect("fabric store")
            .i2c_segment(ctrl, "i2c")
            .expect("claimed segment");
        let addr = I2cAddr::seven(0x20).unwrap();
        let mut ctx = NullCtx;
        segment
            .lock()
            .unwrap()
            .write(&mut ctx, addr, &[0x12, 0x34])
            .expect("write reaches the target face");
        // The declared net exists for the joined line.
        assert!(assembled
            .fabrics
            .as_ref()
            .expect("fabric store")
            .gpio_net("wire:sensor-line")
            .is_some());
        // The face kinds are no longer deferred: they were joined.
        assert!(assembled.deferred.iter().all(|edge| edge.kind != "i2c"));
        assert!(assembled.deferred.iter().all(|edge| edge.kind != "gpio"));
    }

    #[test]
    fn face_edges_without_a_claimed_segment_error_loudly() {
        // A part wired onto another part that never offers a segment: the
        // join must fail, never silently skip.
        let graph = MachineGraph {
            soc_id: "soc:test/slice".into(),
            board_id: Some("board:test".into()),
            nodes: vec![
                GraphNode {
                    id: "board.mux".into(),
                    kind: GraphNodeKind::Part,
                    catalog_id: Some("part:test/mux".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "board.sensor".into(),
                    kind: GraphNodeKind::Part,
                    catalog_id: Some("part:test/sensor".into()),
                    parameters: BTreeMap::new(),
                },
            ],
            regions: Vec::new(),
            edges: vec![GraphEdge {
                id: "face.sensor.i2c".into(),
                from_node: "board.mux".into(),
                from_port: "down2".into(),
                to_node: "board.sensor".into(),
                to_port: "i2c".into(),
                kind: "i2c".into(),
                parameters: [("address".to_owned(), ConfigValue::Integer(0x76))]
                    .into_iter()
                    .collect(),
            }],
        };
        // The mux spawn exposes nothing: it "forgot" its segments.
        fn spawn_lazy_mux(
            _ctx: &InstantiateCtx,
            _machine: &mut mosaic_runtime::Machine,
        ) -> Result<(), MaterializeError> {
            Ok(())
        }
        let mut registry = DeviceRegistry::new();
        registry.register("part:test/mux", spawn_lazy_mux).unwrap();
        registry.register("part:test/sensor", spawn_sensor).unwrap();
        let error = materialize(&graph, &registry, &HostAttachments::new(), 8)
            .expect_err("unclaimed segment must fail");
        assert!(error.to_string().contains("masters no fabric segment"));
    }

    #[test]
    fn offered_segments_take_precedence_for_mux_downstream_faces() {
        // A mux spawn that offers one segment; the sensor attaches there.
        fn spawn_mux(
            ctx: &InstantiateCtx,
            _machine: &mut mosaic_runtime::Machine,
        ) -> Result<(), MaterializeError> {
            ctx.offer_i2c_segment("down2", Arc::new(Mutex::new(I2cBus::new())));
            Ok(())
        }
        let graph = MachineGraph {
            soc_id: "soc:test/slice".into(),
            board_id: Some("board:test".into()),
            nodes: vec![
                GraphNode {
                    id: "board.mux".into(),
                    kind: GraphNodeKind::Part,
                    catalog_id: Some("part:test/mux".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "board.sensor".into(),
                    kind: GraphNodeKind::Part,
                    catalog_id: Some("part:test/sensor".into()),
                    parameters: BTreeMap::new(),
                },
            ],
            regions: Vec::new(),
            edges: vec![GraphEdge {
                id: "face.sensor.i2c".into(),
                from_node: "board.mux".into(),
                from_port: "down2".into(),
                to_node: "board.sensor".into(),
                to_port: "i2c".into(),
                kind: "i2c".into(),
                parameters: [("address".to_owned(), ConfigValue::Integer(0x76))]
                    .into_iter()
                    .collect(),
            }],
        };
        let mut registry = DeviceRegistry::new();
        registry.register("part:test/mux", spawn_mux).unwrap();
        registry.register("part:test/sensor", spawn_sensor).unwrap();
        let assembled = materialize(&graph, &registry, &HostAttachments::new(), 8).unwrap();
        let mux = assembled.nodes["board.mux"];
        let segment = assembled
            .fabrics
            .as_ref()
            .expect("fabric store")
            .i2c_segment(mux, "down2")
            .expect("offered segment");
        let mut ctx = NullCtx;
        // The sensor answered at 0x76 through the mux's offered segment.
        segment
            .lock()
            .unwrap()
            .write(&mut ctx, I2cAddr::seven(0x76).unwrap(), &[0x00])
            .expect("write reaches the sensor through the segment");
    }

    #[test]
    fn spi_faces_join_their_chip_select_target() {
        fn spawn_spi_host(
            ctx: &InstantiateCtx,
            _machine: &mut mosaic_runtime::Machine,
        ) -> Result<(), MaterializeError> {
            let _bus = ctx.spi_segment("spi");
            Ok(())
        }
        fn spawn_spi_io(
            ctx: &InstantiateCtx,
            _machine: &mut mosaic_runtime::Machine,
        ) -> Result<(), MaterializeError> {
            struct SpiRecorder;
            impl SpiTarget for SpiRecorder {
                fn selected(&mut self, _ctx: &mut dyn Ctx, _assert: bool) {}
                fn exchange_byte(
                    &mut self,
                    _ctx: &mut dyn Ctx,
                    _mode: mosaic_contract::spi::SpiMode,
                    _mosi: u8,
                ) -> Result<u8, SpiError> {
                    Ok(0xC3)
                }
            }
            ctx.expose_spi_target("spi", 1, Box::new(SpiRecorder));
            Ok(())
        }
        let graph = MachineGraph {
            soc_id: "soc:test/slice".into(),
            board_id: None,
            nodes: vec![
                GraphNode {
                    id: "soc.spihost".into(),
                    kind: GraphNodeKind::Peripheral,
                    catalog_id: Some("ip:test/spihost".into()),
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "board.io".into(),
                    kind: GraphNodeKind::Part,
                    catalog_id: Some("part:test/io".into()),
                    parameters: BTreeMap::new(),
                },
            ],
            regions: Vec::new(),
            edges: vec![GraphEdge {
                id: "face.io.spi".into(),
                from_node: "soc.spihost".into(),
                from_port: "spi".into(),
                to_node: "board.io".into(),
                to_port: "spi".into(),
                kind: "spi".into(),
                parameters: BTreeMap::new(),
            }],
        };
        let mut registry = DeviceRegistry::new();
        registry
            .register("ip:test/spihost", spawn_spi_host)
            .unwrap();
        registry.register("part:test/io", spawn_spi_io).unwrap();
        let assembled = materialize(&graph, &registry, &HostAttachments::new(), 8).unwrap();
        // Joined spi faces are not deferred either.
        assert!(assembled.deferred.iter().all(|edge| edge.kind != "spi"));
    }
}
