//! Load the STM32F103 SoC declaration from `manifests/machine/arm/`.
//! The SoC grammar is shared by every machine (ADR 0033 wave 3): this
//! module is now a thin `include_str!` over [`crate::manifest::parse_soc`].
//!
//! The strict `region.priority is not used by the STM32F103 map` check the
//! old parser enforced is dropped — `priority` is a documented feature of
//! [`RegionDescriptor`] and the shared parser accepts it; the F103 map
//! simply does not declare any.

use crate::manifest::parse_soc;
use mosaic_describe::SocManifest;

pub fn stm32f103_soc() -> Result<SocManifest, String> {
    parse_soc(include_str!(
        "../../../../../manifests/machine/arm/stm32f103/soc.toml"
    ))
}
