use std::collections::BTreeMap;

use super::*;
use crate::endpoint::parse_endpoint;
use crate::graph::validate_graph;

fn valid_soc(regions: Vec<RegionDescriptor>) -> SocManifest {
    let device = regions
        .iter()
        .find(|region| region.kind == RegionKind::Mmio)
        .map(|region| BlockInstance {
            name: region.target.clone(),
            use_id: "ip:test".into(),
            at: Some(Placement {
                space: "sysbus".into(),
                base: region.base,
            }),
            irq: Vec::new(),
            clk: BTreeMap::new(),
        });
    SocManifest {
        soc: SocIdentity {
            id: "soc:test".into(),
            summary: "test SoC".into(),
        },
        cpu: vec![CpuInstance {
            name: "core0".into(),
            use_id: "cpu:test".into(),
            view: "sysbus".into(),
        }],
        block: device.into_iter().collect(),
        view: vec![AddressSpaceView {
            name: "sysbus".into(),
            region: regions,
        }],
    }
}

fn region(name: &str, base: u64, size: u64, kind: &str, priority: Option<i32>) -> RegionDescriptor {
    RegionDescriptor {
        name: name.into(),
        target: if kind == "mmio" {
            "device".into()
        } else {
            name.into()
        },
        base,
        size,
        kind: match kind {
            "ram" => RegionKind::Ram,
            "rom" => RegionKind::Rom,
            "mmio" => RegionKind::Mmio,
            _ => RegionKind::Ram,
        },
        priority,
    }
}

fn value_map(entries: &[(&str, ConfigValue)]) -> BTreeMap<String, ConfigValue> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

/// A minimal multi-face part: one I2C target face, one GPIO line face and
/// one IRQ output face — the MCP23017 shape (ADR 0032).
fn multi_face_part(name: &str) -> PartInstance {
    PartInstance {
        name: name.into(),
        use_id: "part:test".into(),
        faces: vec![
            PartFace {
                port: "i2c".into(),
                kind: "i2c".into(),
                to: "soc.i2c0:i2c".into(),
                index: None,
                pin: None,
                parameters: value_map(&[("address", ConfigValue::Integer(0x20))]),
            },
            PartFace {
                port: "gpio".into(),
                kind: "gpio".into(),
                to: "wire:expander-line".into(),
                index: Some(4),
                pin: None,
                parameters: BTreeMap::new(),
            },
            PartFace {
                port: "irq".into(),
                kind: "irq".into(),
                to: "wire:expander-int".into(),
                index: None,
                pin: Some("int".into()),
                parameters: BTreeMap::new(),
            },
        ],
    }
}

fn board_with(parts: Vec<PartInstance>) -> BoardManifest {
    BoardManifest {
        board: BoardIdentity {
            id: "board:test".into(),
            soc: Some("soc:test".into()),
            summary: "test board".into(),
        },
        conn: Vec::new(),
        part: parts,
    }
}

#[test]
fn rejects_malformed_catalog_ids_before_graph_construction() {
    let mut manifest = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    manifest.cpu[0].use_id = "ip:arm/pl011".into();
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("namespace"));

    manifest.cpu[0].use_id = "cpu:arm//core".into();
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("invalid path suffix"));
}

#[test]
fn rejects_equal_priority_overlaps_but_allows_explicit_overlays() {
    let overlapping = valid_soc(vec![
        region("ram", 0, 8192, "ram", None),
        region("device", 4096, 4096, "mmio", None),
    ]);
    assert!(overlapping
        .validate()
        .unwrap_err()
        .message
        .contains("overlap"));

    let overlaid = valid_soc(vec![
        region("ram", 0, 8192, "ram", None),
        region("device", 4096, 4096, "mmio", Some(1)),
    ]);
    assert!(overlaid.validate().is_ok());
}

#[test]
fn rejects_empty_regions_and_address_overflow() {
    for invalid in [
        region("ram", 0, 0, "ram", None),
        region("ram", u64::MAX, 2, "ram", None),
    ] {
        assert!(valid_soc(vec![invalid]).validate().is_err());
    }
}

#[test]
fn rejects_mmio_regions_without_a_matching_declared_block() {
    let mut manifest = valid_soc(vec![region("device", 4096, 4096, "mmio", None)]);
    manifest.block.clear();
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("unknown block"));

    let mut manifest = valid_soc(vec![region("device", 4096, 4096, "mmio", None)]);
    manifest.block[0].at.as_mut().unwrap().base = 8192;
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("MMIO region"));

    let mut manifest = valid_soc(vec![region("device", 4096, 4096, "mmio", None)]);
    manifest.block[0].at = None;
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("must match block"));
}

#[test]
fn rejects_duplicate_cpu_block_names_and_unknown_view_references() {
    let mut manifest = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    manifest.block.push(BlockInstance {
        name: "core0".into(),
        use_id: "ip:test".into(),
        at: Some(Placement {
            space: "sysbus".into(),
            base: 0x1000,
        }),
        irq: Vec::new(),
        clk: BTreeMap::new(),
    });
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("duplicate name"));

    let mut manifest = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    manifest.cpu[0].view = "missing".into();
    assert!(manifest
        .validate()
        .unwrap_err()
        .message
        .contains("unknown view"));
}

#[test]
fn connection_names_and_part_names_use_separate_namespaces() {
    let board = BoardManifest {
        board: BoardIdentity {
            id: "board:test".into(),
            soc: Some("soc:test".into()),
            summary: "test board".into(),
        },
        conn: vec![BoardConnection {
            name: "sensor0".into(),
            port: "soc.i2c0".into(),
            to: "wire:sensor0".into(),
            kind: "i2c/endpoint".into(),
        }],
        part: vec![multi_face_part("sensor0")],
    };
    assert!(board.validate().is_ok());
}

#[test]
fn namespace_qualified_irq_endpoints_validate_consistently() {
    let mut manifest = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    manifest.block.push(BlockInstance {
        name: "timer0".into(),
        use_id: "ip:test-timer".into(),
        at: None,
        irq: vec![IrqRoute {
            pin: "timer".into(),
            to: vec!["soc.core0.irq".into()],
        }],
        clk: BTreeMap::new(),
    });
    assert!(manifest.validate().is_ok());
    assert!(MachineGraph::build(&manifest, None).is_ok());
}

#[test]
fn endpoint_parser_rejects_empty_or_malformed_port_segments() {
    assert!(parse_endpoint("soc.core0.", "soc").is_err());
    assert!(parse_endpoint("soc.core0.vic::4", "soc").is_err());
    assert!(parse_endpoint("soc.core0.vic:4", "soc").is_ok());
}

#[test]
fn graph_normalization_is_independent_of_region_declaration_order() {
    let first = valid_soc(vec![
        region("ram", 0, 4096, "ram", None),
        region("rom", 0x10000, 4096, "rom", None),
    ]);
    let second = valid_soc(vec![
        region("rom", 0x10000, 4096, "rom", None),
        region("ram", 0, 4096, "ram", None),
    ]);
    assert_eq!(
        MachineGraph::build(&first, None).unwrap(),
        MachineGraph::build(&second, None).unwrap()
    );
}

#[test]
fn graph_validation_rejects_duplicate_edges_and_wrong_region_targets() {
    let nodes = vec![
        GraphNode {
            id: "view.sysbus".into(),
            kind: GraphNodeKind::AddressSpaceView,
            catalog_id: None,
            parameters: BTreeMap::new(),
        },
        GraphNode {
            id: "soc.core0".into(),
            kind: GraphNodeKind::Cpu,
            catalog_id: Some("cpu:test".into()),
            parameters: BTreeMap::new(),
        },
    ];
    let edge = GraphEdge {
        id: "cpu.space".into(),
        from_node: "soc.core0".into(),
        from_port: "address-space".into(),
        to_node: "view.sysbus".into(),
        to_port: String::new(),
        kind: "address-space".into(),
        parameters: BTreeMap::new(),
    };
    assert!(validate_graph(&nodes, &[], &[edge.clone(), edge])
        .unwrap_err()
        .message
        .contains("duplicate graph edge"));
    let duplicate_nodes = vec![nodes[0].clone(), nodes[0].clone()];
    assert!(validate_graph(&duplicate_nodes, &[], &[])
        .unwrap_err()
        .message
        .contains("duplicate graph node"));
    let invalid_region = GraphRegion {
        name: "ram".into(),
        view: "view.sysbus".into(),
        target_node: "soc.core0".into(),
        base: 0,
        size: 4096,
        kind: RegionKind::Ram,
        priority: None,
    };
    assert!(validate_graph(&nodes, &[invalid_region], &[])
        .unwrap_err()
        .message
        .contains("expected MemoryBacking"));
}

#[test]
fn rejects_unknown_part_face_kinds() {
    let mut part = multi_face_part("sensor0");
    part.faces[0].kind = "i2cc".into();
    let board = board_with(vec![part]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("unsupported face kind"));
}

#[test]
fn rejects_i2c_faces_without_an_initiator_port_or_address() {
    let mut part = multi_face_part("sensor0");
    part.faces[0].to = "soc.i2c0".into();
    let board = board_with(vec![part]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("initiator face"));

    let mut part = multi_face_part("sensor0");
    part.faces[0].parameters.remove("address");
    let board = board_with(vec![part]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("integer address"));

    let mut part = multi_face_part("sensor0");
    part.faces[0]
        .parameters
        .insert("address".into(), ConfigValue::Integer(0x400));
    let board = board_with(vec![part]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("out-of-range I2C address"));
}

#[test]
fn rejects_gpio_faces_without_index_or_net_and_irq_faces_without_pin() {
    let mut part = multi_face_part("sensor0");
    part.faces[1].index = None;
    let board = board_with(vec![part]);
    assert!(board.validate().unwrap_err().message.contains("line index"));

    let mut part = multi_face_part("sensor0");
    part.faces[1].to = "soc.gpioa".into();
    let board = board_with(vec![part]);
    assert!(board.validate().unwrap_err().message.contains("net (wire:"));

    let mut part = multi_face_part("sensor0");
    part.faces[2].pin = None;
    let board = board_with(vec![part]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("output pin name"));
}

#[test]
fn rejects_duplicate_faces_and_duplicate_bus_addresses() {
    let mut part = multi_face_part("sensor0");
    part.faces.push(part.faces[0].clone());
    let board = board_with(vec![part]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("wires face \"i2c\""));

    let second = multi_face_part("sensor1");
    let board = board_with(vec![multi_face_part("sensor0"), second]);
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("duplicate I2C address"));
}

#[test]
fn faces_normalize_into_directed_graph_edges() {
    let mut soc = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    soc.block.push(BlockInstance {
        name: "i2c0".into(),
        use_id: "ip:test-i2c".into(),
        at: None,
        irq: Vec::new(),
        clk: BTreeMap::new(),
    });
    let graph = MachineGraph::build(&soc, Some(&board_with(vec![multi_face_part("ioexp")])))
        .expect("face graph");
    // i2c: initiator → part, with the address riding the parameters.
    let i2c = graph
        .edges
        .iter()
        .find(|edge| edge.kind == "i2c")
        .expect("i2c edge");
    assert_eq!(i2c.from_node, "soc.i2c0");
    assert_eq!(i2c.from_port, "i2c");
    assert_eq!(i2c.to_node, "board.ioexp");
    assert_eq!(i2c.to_port, "i2c");
    assert_eq!(
        i2c.parameters
            .get("address")
            .and_then(ConfigValue::as_integer),
        Some(0x20)
    );
    // gpio: part → net, with the line index riding the parameters.
    let gpio = graph
        .edges
        .iter()
        .find(|edge| edge.kind == "gpio")
        .expect("gpio edge");
    assert_eq!(gpio.from_node, "board.ioexp");
    assert_eq!(gpio.from_port, "gpio");
    assert_eq!(gpio.to_node, "wire:expander-line");
    assert_eq!(
        gpio.parameters
            .get("index")
            .and_then(ConfigValue::as_integer),
        Some(4)
    );
    // irq: part pin → sink, shaped exactly like a block IRQ edge so the
    // existing signal machinery carries it.
    let irq = graph
        .edges
        .iter()
        .find(|edge| edge.kind == "irq")
        .expect("irq edge");
    assert_eq!(irq.from_node, "board.ioexp");
    assert_eq!(irq.from_port, "int");
    assert_eq!(irq.to_node, "wire:expander-int");
}

#[test]
fn mux_segments_are_valid_initiator_endpoints_for_other_parts() {
    let mut soc = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    soc.block.push(BlockInstance {
        name: "i2c0".into(),
        use_id: "ip:test-i2c".into(),
        at: None,
        irq: Vec::new(),
        clk: BTreeMap::new(),
    });
    let board = board_with(vec![
        PartInstance {
            name: "mux".into(),
            use_id: "part:test".into(),
            faces: vec![PartFace {
                port: "i2c".into(),
                kind: "i2c".into(),
                to: "soc.i2c0:i2c".into(),
                index: None,
                pin: None,
                parameters: value_map(&[("address", ConfigValue::Integer(0x70))]),
            }],
        },
        PartInstance {
            name: "sensor".into(),
            use_id: "part:test".into(),
            faces: vec![PartFace {
                port: "i2c".into(),
                kind: "i2c".into(),
                to: "board.mux:down2".into(),
                index: None,
                pin: None,
                parameters: value_map(&[("address", ConfigValue::Integer(0x76))]),
            }],
        },
    ]);
    let graph = MachineGraph::build(&soc, Some(&board)).expect("mux graph");
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == "i2c" && edge.from_node == "board.mux" && edge.from_port == "down2"
    }));
    // The same address on different segments is not a conflict.
    assert!(board.validate().is_ok());
}

#[test]
fn part_clock_faces_accept_block_ports_and_abstract_nets() {
    let mut soc = valid_soc(vec![region("ram", 0, 4096, "ram", None)]);
    soc.block.push(BlockInstance {
        name: "i2c0".into(),
        use_id: "ip:test-i2c".into(),
        at: None,
        irq: Vec::new(),
        clk: BTreeMap::new(),
    });
    let mut part = multi_face_part("clocked");
    part.faces.truncate(1);
    part.faces.push(PartFace {
        port: "clock".into(),
        kind: "clock".into(),
        to: "rcc.gate-apb1-i2c1".into(),
        index: None,
        pin: None,
        parameters: BTreeMap::new(),
    });
    soc.block.push(BlockInstance {
        name: "rcc".into(),
        use_id: "ip:test-rcc".into(),
        at: None,
        irq: Vec::new(),
        clk: BTreeMap::new(),
    });
    let graph = MachineGraph::build(&soc, Some(&board_with(vec![part]))).expect("clock face");
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == "clock"
            && edge.from_node == "soc.rcc"
            && edge.from_port == "gate-apb1-i2c1"
            && edge.to_node == "board.clocked"
            && edge.to_port == "clock"
    }));
}
