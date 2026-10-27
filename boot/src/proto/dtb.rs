//! Device tree blob (FDT) parsing and loader-side fixups.
//!
//! Clean-room from the Devicetree Specification 0.4 structure block: the
//! big-endian header (magic `0xD00DFEED`, totalsize, off_mem_rsvmap,
//! off_dt_struct), the token stream (`FDT_BEGIN_NODE`, `FDT_END_NODE`,
//! `FDT_PROP`, `FDT_NOP`, `FDT_END`), and the `u32` cell encoding of
//! `/chosen`'s `bootargs`, `initrd-start` and `initrd-end`.
//!
//! A boot loader's documented DTB jobs are supported: find nodes and
//! properties, rewrite a property in place (same byte length), and read
//! cells. *Growing* a DTB is a builder's job, not a loader's — an absent
//! property is reported, never invented.

use crate::loader::LoaderError;

/// One located property: `(node path, property name, byte range)`.
type WalkedProperties = Vec<(String, String, (usize, usize))>;

/// The FDT magic, big-endian.
pub const MAGIC: u32 = 0xD00D_FEED;

const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_NOP: u32 = 0x0000_0004;
const FDT_END: u32 = 0x0000_0009;

/// Why a DTB access failed.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FdtError {
    BadMagic,
    Truncated,
    UnknownToken {
        token: u32,
    },
    NoSuchNode {
        path: String,
    },
    NoSuchProperty {
        node: String,
        property: String,
    },
    /// The new value does not fit the existing property's byte length.
    DoesNotFit {
        needed: usize,
        have: usize,
    },
    /// A cell property was decoded with a length that is not a multiple of
    /// four.
    NotCells {
        len: usize,
    },
}

impl std::fmt::Display for FdtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "DTB: bad magic"),
            Self::Truncated => write!(f, "DTB: truncated structure block"),
            Self::UnknownToken { token } => write!(f, "DTB: unknown token {token:#010x}"),
            Self::NoSuchNode { path } => write!(f, "DTB: no node {path}"),
            Self::NoSuchProperty { node, property } => {
                write!(f, "DTB: no property {property:?} in {node}")
            }
            Self::DoesNotFit { needed, have } => {
                write!(f, "DTB: fixup needs {needed} bytes, property has {have}")
            }
            Self::NotCells { len } => write!(f, "DTB: property of {len} bytes is not cells"),
        }
    }
}

impl std::error::Error for FdtError {}

impl From<FdtError> for LoaderError {
    fn from(error: FdtError) -> Self {
        LoaderError::Unsupported {
            format: "DTB",
            detail: error.to_string(),
        }
    }
}

/// A parsed device tree blob: the byte image plus the located structure
/// block. The blob is borrowed; fixups go through [`Fdt::fixup`], which
/// rewrites in place inside a `&mut [u8]` blob.
#[derive(Debug)]
pub struct Fdt<'a> {
    blob: &'a [u8],
    /// Bounds of the structure block's token stream.
    struct_block: (usize, usize),
    /// Base the property-name offsets are relative to (off_dt_strings).
    strings_base: usize,
}

impl<'a> Fdt<'a> {
    /// Parse the header and locate the structure block.
    pub fn parse(blob: &'a [u8]) -> Result<Self, FdtError> {
        if blob.len() < 40 {
            return Err(FdtError::Truncated);
        }
        let be32 =
            |at: usize| u32::from_be_bytes([blob[at], blob[at + 1], blob[at + 2], blob[at + 3]]);
        if be32(0) != MAGIC {
            return Err(FdtError::BadMagic);
        }
        let totalsize = be32(4) as usize;
        if totalsize < 40 || totalsize > blob.len() {
            return Err(FdtError::Truncated);
        }
        let struct_start = be32(8) as usize;
        if struct_start < 40 || struct_start > totalsize {
            return Err(FdtError::Truncated);
        }
        // off_dt_strings: the strings block's own base, and the structure
        // block's usual upper bound (the token stream itself ends at FDT_END).
        let strings_base = be32(12) as usize;
        if strings_base < 40 || strings_base > totalsize {
            return Err(FdtError::Truncated);
        }
        Ok(Self {
            blob: &blob[..totalsize],
            struct_block: (struct_start, totalsize),
            strings_base,
        })
    }

    /// Walk the token stream, collecting every node's full path (leading
    /// `/` included; the root is `"/"`) and its properties' byte ranges.
    fn walk(&self) -> Result<WalkedProperties, FdtError> {
        let (start, end) = self.struct_block;
        let mut at = start;
        let mut path: Vec<String> = Vec::new();
        let mut nodes: Vec<(String, String, (usize, usize))> = Vec::new();
        let mut depth = 0usize;
        while at + 4 <= end {
            let token = u32::from_be_bytes([
                self.blob[at],
                self.blob[at + 1],
                self.blob[at + 2],
                self.blob[at + 3],
            ]);
            match token {
                FDT_BEGIN_NODE => {
                    at += 4;
                    let name_start = at;
                    let nul = self.blob[name_start..end]
                        .iter()
                        .position(|byte| *byte == 0)
                        .ok_or(FdtError::Truncated)?;
                    let name = String::from_utf8_lossy(&self.blob[name_start..name_start + nul])
                        .to_string();
                    at = (name_start + nul + 1 + 3) & !3;
                    depth += 1;
                    if depth == 1 {
                        path.clear();
                    }
                    path.push(name);
                }
                FDT_END_NODE => {
                    at += 4;
                    depth = depth.saturating_sub(1);
                    path.pop();
                }
                FDT_PROP => {
                    if at + 12 > end {
                        return Err(FdtError::Truncated);
                    }
                    let len = u32::from_be_bytes([
                        self.blob[at + 4],
                        self.blob[at + 5],
                        self.blob[at + 6],
                        self.blob[at + 7],
                    ]) as usize;
                    let nameoff = u32::from_be_bytes([
                        self.blob[at + 8],
                        self.blob[at + 9],
                        self.blob[at + 10],
                        self.blob[at + 11],
                    ]) as usize;
                    let data_start = at + 12;
                    let data_end = data_start
                        .checked_add(len)
                        .filter(|end| *end <= self.struct_block.1)
                        .ok_or(FdtError::Truncated)?;
                    // Property names are offsets into the *strings block*
                    // (off_dt_strings), not absolute blob offsets.
                    let name_at = self.strings_base + nameoff;
                    let name_nul = self.blob[name_at..]
                        .iter()
                        .position(|byte| *byte == 0)
                        .ok_or(FdtError::Truncated)?;
                    let name = String::from_utf8_lossy(&self.blob[name_at..name_at + name_nul])
                        .to_string();
                    // The root's own name is empty; join the children only.
                    let node = if path.len() <= 1 {
                        "/".to_string()
                    } else {
                        format!("/{}", path[1..].join("/"))
                    };
                    nodes.push((node, name, (data_start, data_end)));
                    at = (data_end + 3) & !3;
                }
                FDT_NOP => at += 4,
                FDT_END => break,
                other => return Err(FdtError::UnknownToken { token: other }),
            }
        }
        Ok(nodes)
    }

    /// Find a property's byte range by node path (`/chosen`) and name.
    pub fn property(&self, node: &str, name: &str) -> Result<(usize, usize), FdtError> {
        self.walk()?
            .into_iter()
            .find(|(node_path, property, _)| node_path == node && property == name)
            .map(|(_, _, range)| range)
            .ok_or(FdtError::NoSuchProperty {
                node: node.into(),
                property: name.into(),
            })
    }

    /// Read a property as `u32` cells (big-endian, per the spec).
    pub fn cells(&self, node: &str, name: &str) -> Result<Vec<u32>, FdtError> {
        let (start, end) = self.property(node, name)?;
        let len = end - start;
        if len % 4 != 0 {
            return Err(FdtError::NotCells { len });
        }
        let mut cells = Vec::with_capacity(len / 4);
        for at in (start..end).step_by(4) {
            cells.push(u32::from_be_bytes([
                self.blob[at],
                self.blob[at + 1],
                self.blob[at + 2],
                self.blob[at + 3],
            ]));
        }
        Ok(cells)
    }

    /// Read a string property (NUL-terminated).
    pub fn string(&self, node: &str, name: &str) -> Result<String, FdtError> {
        let (start, end) = self.property(node, name)?;
        let bytes = &self.blob[start..end];
        let until_nul = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..until_nul]).to_string())
    }

    /// The blob's total size, as the header records it.
    pub fn total_size(&self) -> usize {
        self.blob.len()
    }
}

/// Rewrite a property's bytes in place inside a whole DTB blob.
///
/// The new value must fit the existing property exactly (the devicetree
/// format has no length slack; growing requires a rebuild, which a loader
/// does not do). This is the fixup a bootloader performs on `/chosen`.
pub fn fixup(blob: &mut [u8], node: &str, name: &str, new_value: &[u8]) -> Result<(), FdtError> {
    // Locate first (immutable walk), then rewrite through the same range.
    let (start, end) = {
        let fdt = Fdt::parse(blob)?;
        fdt.property(node, name)?
    };
    let len = end - start;
    if new_value.len() != len {
        return Err(FdtError::DoesNotFit {
            needed: new_value.len(),
            have: len,
        });
    }
    blob[start..end].copy_from_slice(new_value);
    Ok(())
}

/// Encode a value as big-endian `u32` cells (the `/chosen` cell encoding).
pub fn cells_be(values: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one minimal DTB: `/` with `/chosen` inside, properties
    /// `bootargs` (a string) and `initrd-start`/`initrd-end` (cells). A
    /// *format fixture* for the parser.
    fn dtb() -> Vec<u8> {
        let mut strings = Vec::new();
        let string = |text: &str, strings: &mut Vec<u8>| -> u32 {
            let offset = strings.len() as u32;
            strings.extend_from_slice(text.as_bytes());
            strings.push(0);
            offset
        };
        let bootargs = string("bootargs", &mut strings);
        let initrd_start = string("initrd-start", &mut strings);
        let initrd_end = string("initrd-end", &mut strings);

        let mut structure = Vec::new();
        let token = |structure: &mut Vec<u8>, value: u32| {
            structure.extend_from_slice(&value.to_be_bytes());
        };
        let node_name = |structure: &mut Vec<u8>, name: &str| {
            structure.extend_from_slice(name.as_bytes());
            structure.push(0);
            while structure.len() % 4 != 0 {
                structure.push(0);
            }
        };
        token(&mut structure, FDT_BEGIN_NODE);
        node_name(&mut structure, "");
        token(&mut structure, FDT_BEGIN_NODE);
        node_name(&mut structure, "chosen");
        for (nameoff, data) in [
            (bootargs, b"console=ttyS0\0".to_vec()),
            (initrd_start, cells_be(&[0x8100_0000])),
            (initrd_end, cells_be(&[0x8100_1000])),
        ] {
            token(&mut structure, FDT_PROP);
            structure.extend_from_slice(&(data.len() as u32).to_be_bytes());
            structure.extend_from_slice(&nameoff.to_be_bytes());
            structure.extend_from_slice(&data);
            while structure.len() % 4 != 0 {
                structure.push(0);
            }
        }
        token(&mut structure, FDT_END_NODE);
        token(&mut structure, FDT_END_NODE);
        token(&mut structure, FDT_END);

        let header_size = 40;
        let struct_offset = header_size;
        let strings_offset = struct_offset + structure.len();
        let mut blob = vec![0u8; header_size];
        blob[0..4].copy_from_slice(&MAGIC.to_be_bytes());
        blob[4..8].copy_from_slice(&((strings_offset + strings.len()) as u32).to_be_bytes());
        blob[8..12].copy_from_slice(&(struct_offset as u32).to_be_bytes());
        blob[12..16].copy_from_slice(&(strings_offset as u32).to_be_bytes());
        blob.extend_from_slice(&structure);
        blob.extend_from_slice(&strings);
        blob
    }

    #[test]
    fn the_structure_block_walks_and_finds_chosen() {
        let bytes = dtb();
        let fdt = Fdt::parse(&bytes).unwrap();
        assert_eq!(fdt.string("/chosen", "bootargs").unwrap(), "console=ttyS0");
        assert_eq!(
            fdt.cells("/chosen", "initrd-start").unwrap(),
            vec![0x8100_0000]
        );
        assert_eq!(
            fdt.cells("/chosen", "initrd-end").unwrap(),
            vec![0x8100_1000]
        );
    }

    #[test]
    fn a_missing_property_is_reported_not_invented() {
        let bytes = dtb();
        let fdt = Fdt::parse(&bytes).unwrap();
        assert_eq!(
            fdt.string("/chosen", "stdout-path"),
            Err(FdtError::NoSuchProperty {
                node: "/chosen".into(),
                property: "stdout-path".into()
            })
        );
    }

    #[test]
    fn a_loader_fixup_rewrites_the_same_length_in_place() {
        let mut bytes = dtb();
        fixup(&mut bytes, "/chosen", "bootargs", b"console=ttyS1\0").unwrap();
        let fdt = Fdt::parse(&bytes).unwrap();
        assert_eq!(fdt.string("/chosen", "bootargs").unwrap(), "console=ttyS1");
    }

    #[test]
    fn a_fixup_that_would_grow_is_rejected() {
        let mut bytes = dtb();
        assert_eq!(
            fixup(&mut bytes, "/chosen", "bootargs", b"console=ttyS0,115200\0"),
            Err(FdtError::DoesNotFit {
                needed: 21,
                have: 14
            })
        );
    }

    #[test]
    fn garbage_is_rejected_by_magic() {
        assert_eq!(Fdt::parse(&[0u8; 64]).unwrap_err(), FdtError::BadMagic);
    }
}
