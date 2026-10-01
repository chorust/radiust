//! Shared, versioned RDCAP reflectivity legend for decoding and rendering.

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RdcapPalette {
    pub schema_version: u8,
    pub id: String,
    pub version: String,
    pub source: String,
    pub product: String,
    pub units: String,
    pub classification: String,
    pub under_threshold: UnderThreshold,
    pub missing_rgba: [u8; 4],
    pub annotation: Annotation,
    pub evidence: Evidence,
    pub classes: Vec<PaletteClass>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnderThreshold {
    pub less_than_dbz: f32,
    pub rgba: [u8; 4],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Annotation {
    pub raw_value: i16,
    pub quality: u16,
    pub rgba: [u8; 4],
    pub role: String,
    pub confidence: String,
    pub rule_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Evidence {
    pub reference: String,
    pub samples: Vec<String>,
    pub limitation: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PaletteClass {
    pub lower_dbz: f32,
    pub raw_lower_threshold: i16,
    pub rgb: [u8; 3],
}

pub(crate) fn palette() -> &'static RdcapPalette {
    static PALETTE: OnceLock<RdcapPalette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        let palette: RdcapPalette =
            serde_json::from_str(include_str!("../resources/rdcap_reflectivity.json"))
                .expect("checked-in RDCAP palette resource is valid");
        assert_eq!(palette.schema_version, 1);
        assert_eq!(palette.source, "rdcap");
        assert_eq!(palette.product, "reflectivity");
        assert_eq!(palette.units, "dBZ");
        assert_eq!(palette.classes.len(), 15);
        assert!(palette.classes.windows(2).all(|pair| pair[0].lower_dbz < pair[1].lower_dbz));
        palette
    })
}

#[cfg(test)]
mod tests {
    use super::palette;

    #[test]
    fn compiled_and_python_palette_resources_are_the_same_versioned_data() {
        let rust_resource = include_str!("../resources/rdcap_reflectivity.json");
        let python_resource =
            include_str!("../../../python/radiust/resources/palettes/rdcap_reflectivity.json");
        let rust: serde_json::Value = serde_json::from_str(rust_resource).unwrap();
        let python: serde_json::Value = serde_json::from_str(python_resource).unwrap();
        assert_eq!(rust, python);
        assert_eq!(palette().id, "rdcap-reflectivity-v1");
        assert_eq!(palette().annotation.quality, 65);
        assert_eq!(palette().classes[0].rgb, [99, 82, 115]);
        assert_eq!(palette().classes[14].rgb, [255, 255, 255]);
    }
}
