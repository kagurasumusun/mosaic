//! Load the AArch64 `virt64` SoC declaration from `manifests/machine/arm/`.
//! The SoC grammar is shared by every machine (ADR 0033 wave 3): this
//! module is a thin `include_str!` over [`crate::manifest::parse_soc`].

use crate::manifest::parse_soc;
use mosaic_describe::SocManifest;

pub fn virt64_soc() -> Result<SocManifest, String> {
    parse_soc(include_str!(
        "../../../../../manifests/machine/arm/virt64/soc.toml"
    ))
}
