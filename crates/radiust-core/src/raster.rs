//! Shared models for pixel-space reflectivity and stable raster inputs.

use crate::model::{FrameRef, Grid, RadarDataset, RadarField, RawFrameReceipt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub const RASTER_SCHEMA_VERSION: u8 = 1;
pub const PIXEL_DBZ_PROFILE: &str = "pixel-dbz-v1";
pub const LOCAL_GRAY_IDENTITY_DOMAIN: &str = "radiust-local-gray-v1";
pub const LOCAL_NUMERIC_IDENTITY_DOMAIN: &str = "radiust-local-numeric-v1";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RasterInput {
    Source {
        frame: FrameRef,
        resolved_revision: String,
        acquisition_receipt: RawFrameReceipt,
    },
    Local {
        identity: RasterInputIdentity,
        read_receipt: InputReadReceipt,
    },
    NumericFile {
        identity: NumericFileIdentity,
        read_receipt: NumericReadReceipt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upstream_provenance: Option<UpstreamProvenance>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RasterInputIdentity {
    pub kind: String,
    pub content_sha256: String,
    pub encoding_declared: String,
    #[serde(default)]
    pub valid_time: Option<String>,
    #[serde(default)]
    pub geometry: Option<GeometryEvidence>,
}

impl RasterInputIdentity {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.kind != "local_gray" || self.encoding_declared != "gray-dbz-v1" {
            return Err("local gray identity kind or encoding is invalid");
        }
        validate_sha256(&self.content_sha256)?;
        validate_optional_time(self.valid_time.as_deref())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NumericFileIdentity {
    pub kind: String,
    pub format: String,
    pub content_digest: String,
    pub variable: String,
    #[serde(default)]
    pub selection: Value,
    #[serde(default)]
    pub valid_time: Option<String>,
    #[serde(default)]
    pub geometry: Option<GeometryEvidence>,
}

impl NumericFileIdentity {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.kind != "local_numeric" || self.format.is_empty() || self.variable.trim().is_empty()
        {
            return Err("numeric file identity is incomplete");
        }
        validate_sha256(&self.content_digest)?;
        validate_optional_time(self.valid_time.as_deref())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FileComponentDigest {
    pub role: String,
    pub relative_key: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InputReadReceipt {
    pub content_sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub source_bit_depth: u8,
    #[serde(default)]
    pub source_dtype: String,
    pub channels: u8,
    #[serde(default)]
    pub frame_index: Option<u32>,
}

impl InputReadReceipt {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_sha256(&self.content_sha256)?;
        if self.size_bytes == 0 || self.width == 0 || self.height == 0 {
            return Err("local read receipt has empty size or shape");
        }
        if !matches!(self.source_bit_depth, 8 | 16 | 32 | 64)
            || !matches!(self.channels, 1 | 2 | 3 | 4)
            || self.source_dtype != source_dtype_for_depth(self.source_bit_depth)
        {
            return Err("local read receipt bit depth or channels are unsupported");
        }
        Ok(())
    }
}

fn source_dtype_for_depth(bit_depth: u8) -> &'static str {
    match bit_depth {
        8 => "u8",
        16 => "u16",
        32 => "f32",
        64 => "f64",
        _ => "",
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NumericReadReceipt {
    pub content_digest: String,
    pub format: String,
    pub size_bytes: u64,
    pub variable: String,
    #[serde(default)]
    pub components: Vec<FileComponentDigest>,
    #[serde(default)]
    pub selection: Value,
    #[serde(default)]
    pub units: Option<String>,
    #[serde(default)]
    pub validated_schema: Option<String>,
}

impl NumericReadReceipt {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_sha256(&self.content_digest)?;
        if self.format.is_empty() || self.variable.trim().is_empty() || self.size_bytes == 0 {
            return Err("numeric read receipt is incomplete");
        }
        let mut previous: Option<(&str, &str)> = None;
        for component in &self.components {
            if component.role.is_empty()
                || component.relative_key.is_empty()
                || component.relative_key.starts_with('/')
                || component.relative_key.contains('\\')
                || component.relative_key.split('/').any(|part| part == ".." || part.is_empty())
            {
                return Err("numeric receipt contains an unsafe component key");
            }
            validate_sha256(&component.sha256)?;
            let key = (component.role.as_str(), component.relative_key.as_str());
            if previous.is_some_and(|value| value >= key) {
                return Err("numeric receipt components are not uniquely sorted");
            }
            previous = Some(key);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UpstreamProvenance {
    #[serde(default)]
    pub input_identity: Option<Value>,
    #[serde(default)]
    pub processing_record: Option<Value>,
    #[serde(default)]
    pub manifest_output_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EncodingBasis {
    VerifiedSourceRule { encoding_id: String, encoding_version: u16, rule: GrayRuleIdentity },
    UserDeclaration { encoding_id: String, encoding_version: u16 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GrayRuleIdentity {
    pub source: String,
    pub product: String,
    pub path_id: String,
    #[serde(default)]
    pub station: Option<String>,
    pub rule_version: String,
    pub config_hash: String,
    pub evidence_ref: String,
    pub encoding_wire_version: String,
    #[serde(default)]
    pub input_constraints: Value,
    #[serde(default)]
    pub ordered_steps: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GrayDecision {
    Applied(GrayFrame),
    Unavailable {
        reason: String,
        #[serde(default)]
        original_preview: Option<Vec<u8>>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GrayFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    #[serde(default)]
    pub frame_index: Option<u32>,
    #[serde(default)]
    pub alpha: Option<AlphaPlane>,
    pub quality: Vec<u16>,
    #[serde(default)]
    pub origin_quality: Option<Vec<u16>>,
    pub input: RasterInput,
    pub encoding_basis: EncodingBasis,
    #[serde(default)]
    pub valid_time: Option<String>,
    #[serde(default)]
    pub geometry: Option<GeometryEvidence>,
}

impl GrayFrame {
    pub fn validate(&self) -> Result<(), &'static str> {
        let pixels = checked_pixel_count(self.height as usize, self.width as usize)?;
        if self.rgba.len() != pixels.checked_mul(4).ok_or("gray shape overflows")? {
            return Err("gray RGBA does not match its shape");
        }
        if self.quality.len() != pixels
            || self.origin_quality.as_ref().is_some_and(|values| values.len() != pixels)
        {
            return Err("gray quality arrays do not match their shape");
        }
        if let Some(alpha) = &self.alpha {
            alpha.validate(pixels)?;
        }
        validate_optional_time(self.valid_time.as_deref())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "bit_depth", content = "values", rename_all = "snake_case")]
pub enum AlphaPlane {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

impl AlphaPlane {
    pub fn bit_depth(&self) -> u8 {
        match self {
            Self::U8(_) => 8,
            Self::U16(_) => 16,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::U16(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_zero(&self, index: usize) -> Option<bool> {
        match self {
            Self::U8(v) => v.get(index).map(|value| *value == 0),
            Self::U16(v) => v.get(index).map(|value| *value == 0),
        }
    }

    pub fn validate(&self, pixels: usize) -> Result<(), &'static str> {
        if self.len() != pixels {
            return Err("alpha does not match raster shape");
        }
        Ok(())
    }

    /// Display-only conversion. Non-zero 16-bit alpha remains visible.
    pub fn preview_u8(&self) -> Vec<u8> {
        match self {
            Self::U8(values) => values.clone(),
            Self::U16(values) => values
                .iter()
                .map(|value| {
                    if *value == 0 {
                        0
                    } else {
                        (((u32::from(*value) * 255 + 32_767) / 65_535).max(1)) as u8
                    }
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PixelDbzField {
    pub variable: String,
    pub units: String,
    pub width: usize,
    pub height: usize,
    pub values: Vec<f32>,
    pub quality: Vec<u16>,
    #[serde(default)]
    pub origin_quality: Option<Vec<u16>>,
    #[serde(default)]
    pub encoding_adjustment: Option<Vec<u8>>,
    #[serde(default)]
    pub alpha: Option<AlphaPlane>,
    #[serde(default)]
    pub valid_time: Option<String>,
    #[serde(default)]
    pub geometry: Option<GeometryEvidence>,
    pub processing: ProcessingRecord,
}

impl PixelDbzField {
    pub fn validate(&self) -> Result<(), &'static str> {
        let pixels = checked_pixel_count(self.height, self.width)?;
        if self.variable != "reflectivity" || self.units != "dBZ" {
            return Err("pixel field must be reflectivity in dBZ");
        }
        if self.values.len() != pixels
            || self.quality.len() != pixels
            || self.origin_quality.as_ref().is_some_and(|v| v.len() != pixels)
            || self.encoding_adjustment.as_ref().is_some_and(|v| v.len() != pixels)
        {
            return Err("pixel dBZ arrays do not match their shape");
        }
        if self
            .values
            .iter()
            .zip(&self.quality)
            .any(|(value, quality)| value.is_infinite() || (value.is_nan() && *quality == 0))
        {
            return Err("non-finite values require a nonzero quality flag");
        }
        if let Some(alpha) = &self.alpha {
            alpha.validate(pixels)?;
        }
        if self.processing.alpha_bit_depth != self.alpha.as_ref().map(AlphaPlane::bit_depth) {
            return Err("processing alpha bit depth does not match the alpha array");
        }
        validate_optional_time(self.valid_time.as_deref())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GeometryEvidence {
    pub source: String,
    pub crs: Option<String>,
    #[serde(default)]
    pub x: Vec<f64>,
    #[serde(default)]
    pub y: Vec<f64>,
    #[serde(default)]
    pub affine: Option<[f64; 6]>,
    #[serde(default)]
    pub mapping_complete: bool,
}

impl GeometryEvidence {
    /// Materialize grid metadata only when the complete pixel-to-coordinate
    /// mapping is present. This clones coordinate axes, never raster values.
    pub fn trusted_grid(&self, width: usize, height: usize) -> Option<Grid> {
        (self.mapping_complete
            && crate::grid::has_complete_pixel_mapping(
                width,
                height,
                self.crs.as_deref(),
                &self.x,
                &self.y,
            ))
        .then(|| Grid {
            shape: vec![height, width],
            crs: self.crs.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            affine: self.affine,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProcessingRecord {
    pub schema_version: u8,
    pub method: String,
    pub input_identity: Value,
    #[serde(default)]
    pub encoding_basis: Option<EncodingBasis>,
    #[serde(default)]
    pub range_policy: Option<String>,
    #[serde(default)]
    pub decoder_version: Option<String>,
    #[serde(default)]
    pub quality_policy_version: Option<String>,
    #[serde(default)]
    pub formula: Option<String>,
    #[serde(default)]
    pub quantization_step: Option<f32>,
    #[serde(default)]
    pub alpha_bit_depth: Option<u8>,
    #[serde(default)]
    pub steps: Vec<Value>,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub clipped_pixel_count: Option<u64>,
    #[serde(default)]
    pub valid_clipped_pixel_count: Option<u64>,
    #[serde(default)]
    pub upstream: Option<UpstreamProvenance>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModeInfo {
    #[serde(default)]
    pub requested: Option<String>,
    #[serde(default)]
    pub actual: Option<String>,
    #[serde(default)]
    pub variable: Option<String>,
    #[serde(default)]
    pub units: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub rule_version: Option<String>,
    #[serde(default)]
    pub range_policy: Option<String>,
    #[serde(default)]
    pub clipped_pixel_count: Option<u64>,
    #[serde(default)]
    pub valid_clipped_pixel_count: Option<u64>,
    #[serde(default)]
    pub time_status: Option<String>,
    #[serde(default)]
    pub geolocation: Option<String>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum RasterResultData {
    Native(Arc<RadarField>),
    NativeDataset { owner: Arc<RadarDataset>, index: usize },
    Pixel(Arc<PixelDbzField>),
}

#[derive(Clone, Debug)]
pub struct RasterResult {
    pub input: RasterInput,
    pub data: RasterResultData,
    pub processing: ProcessingRecord,
    pub mode_info: ModeInfo,
}

#[derive(Clone, Copy, Debug)]
pub enum RasterViewData<'a> {
    Native(&'a RadarField),
    NativeDataset { owner: &'a RadarDataset, index: usize },
    Pixel(&'a PixelDbzField),
}

#[derive(Clone, Copy, Debug)]
pub struct RasterView<'a> {
    pub input: &'a RasterInput,
    pub data: RasterViewData<'a>,
    pub processing: &'a ProcessingRecord,
    pub mode_info: &'a ModeInfo,
}

pub const QUALITY_MISSING: u16 = 1;
pub const QUALITY_OUTSIDE_COVERAGE: u16 = 2;
pub const QUALITY_UNKNOWN_COLOR: u16 = 4;
pub const QUALITY_RECOVERED: u16 = 8;
pub const QUALITY_INTERPOLATED: u16 = 16;
pub const QUALITY_BELOW_DETECTION: u16 = 32;
pub const QUALITY_SOURCE_ANNOTATION: u16 = 64;
pub const QUALITY_FLAG_MASKS: [u16; 7] = [
    QUALITY_MISSING,
    QUALITY_OUTSIDE_COVERAGE,
    QUALITY_UNKNOWN_COLOR,
    QUALITY_RECOVERED,
    QUALITY_INTERPOLATED,
    QUALITY_BELOW_DETECTION,
    QUALITY_SOURCE_ANNOTATION,
];
pub const QUALITY_FLAG_MEANINGS: [&str; 7] = [
    "missing",
    "outside_coverage",
    "unknown_color",
    "recovered",
    "interpolated",
    "below_detection",
    "source_annotation",
];

pub fn checked_pixel_count(height: usize, width: usize) -> Result<usize, &'static str> {
    if height == 0 || width == 0 {
        return Err("pixel shape dimensions must be positive");
    }
    height.checked_mul(width).ok_or("pixel shape overflows")
}

fn validate_sha256(value: &str) -> Result<(), &'static str> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("content digest must be a SHA-256 hex string");
    }
    Ok(())
}

fn validate_optional_time(value: Option<&str>) -> Result<(), &'static str> {
    if let Some(value) = value {
        crate::model::parse_utc_time(value).map_err(|_| "valid time must be RFC3339")?;
    }
    Ok(())
}
