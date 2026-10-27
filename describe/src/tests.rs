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
        part: vec![PartInstance {
            name: "sensor0".into(),
            use_id: "part:test".into(),
            bind: "i2c".into(),
            conn: value_map(&[
                ("bus", ConfigValue::String("soc.i2c0".into())),
                ("addr", ConfigValue::Integer(0x48)),
            ]),
        }],
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
fn rejects_unknown_part_binding_kinds() {
    let board = BoardManifest {
        board: BoardIdentity {
            id: "board:test".into(),
            soc: Some("soc:test".into()),
            summary: "test board".into(),
        },
        conn: Vec::new(),
        part: vec![PartInstance {
            name: "sensor0".into(),
            use_id: "part:test".into(),
            bind: "i2cc".into(),
            conn: value_map(&[("bus", ConfigValue::String("i2c0".into()))]),
        }],
    };
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("unsupported binding kind"));
}

#[test]
fn validates_bus_address_and_chip_select_uniqueness() {
    let part = |name: &str, bind: &str, conn: BTreeMap<String, ConfigValue>| PartInstance {
        name: name.into(),
        use_id: "part:test".into(),
        bind: bind.into(),
        conn,
    };
    let board = BoardManifest {
        board: BoardIdentity {
            id: "board:test".into(),
            soc: Some("soc:test".into()),
            summary: "test board".into(),
        },
        conn: Vec::new(),
        part: vec![
            part(
                "sensor0",
                "i2c",
                value_map(&[
                    ("bus", ConfigValue::String("i2c0".into())),
                    ("addr", ConfigValue::Integer(0x48)),
                ]),
            ),
            part(
                "sensor1",
                "i2c",
                value_map(&[
                    ("bus", ConfigValue::String("i2c0".into())),
                    ("addr", ConfigValue::Integer(0x48)),
                ]),
            ),
        ],
    };
    assert!(board
        .validate()
        .unwrap_err()
        .message
        .contains("duplicate I2C address"));
}
