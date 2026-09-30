//! Native Zarr v2 output using the same Blosc/LZ4 and chunk layout as the
//! Python xarray writer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use zarrs::array::{Array, ArrayMetadataOptions, CodecOptions, Element, ElementOwned};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::Group;
use zarrs::metadata::FillValueMetadata;
use zarrs::metadata::v2::{ArrayMetadataV2, GroupMetadataV2};
use zarrs::storage::{ListableStorageTraits, ReadableStorageTraits};

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{RadarDataset, RadarField, parse_utc_time};

const CHUNK_EDGE: usize = 512;
const QUALITY_FLAGS: [u16; 6] = [1, 2, 4, 8, 16, 32];
const QUALITY_MEANINGS: &str =
    "missing outside_coverage unknown_color recovered interpolated below_detection";

fn storage_error(message: impl Into<String>) -> CoreError {
    CoreError::Storage(message.into())
}

/// Write one field to a consolidated Zarr v2 directory store.
///
/// Scientific values and quality flags remain `float32` and `uint16`; arrays
/// use the existing 512-element chunk ceiling and Blosc/LZ4 level 5 codec.
/// The completed directory is published only after all metadata and chunks
/// have been written and consolidated.
pub fn write_field(
    field: &RadarField,
    destination: impl AsRef<Path>,
    limits: &Limits,
) -> CoreResult<PathBuf> {
    field.validate().map_err(|error| storage_error(error.to_string()))?;
    write_fields(std::slice::from_ref(field), destination.as_ref(), limits)
}

/// Write a shared-grid, shared-time dataset to a consolidated Zarr v2 store.
pub fn write_dataset(
    dataset: &RadarDataset,
    destination: impl AsRef<Path>,
    limits: &Limits,
) -> CoreResult<PathBuf> {
    dataset.validate().map_err(|error| storage_error(error.to_string()))?;
    write_fields(&dataset.fields, destination.as_ref(), limits)
}

/// Read one field from a consolidated Zarr v2 directory written in Radiust's
/// supported layout. Multi-variable stores require an explicit variable name.
pub fn read_selected_field(
    path: impl AsRef<Path>,
    variable: Option<&str>,
    limits: &Limits,
) -> CoreResult<RadarField> {
    if variable.is_some_and(|name| !valid_array_name(name)) {
        return Err(storage_error("Zarr variable name is invalid"));
    }

    let (store_path, _) = inspect_input_directory(path.as_ref(), limits)?;
    let (group_attrs, descriptors) = read_consolidated_metadata(&store_path)?;
    let grid_kind = required_string(&group_attrs, "radiust_grid_kind")?;
    if grid_kind != "geographic" && grid_kind != "cartesian" {
        return Err(storage_error("Zarr grid kind is unsupported"));
    }
    let _: Vec<String> = serde_json::from_value(
        group_attrs
            .get("radiust_provenance")
            .cloned()
            .ok_or_else(|| storage_error("Zarr root provenance metadata is missing"))?,
    )
    .map_err(|_| storage_error("Zarr root provenance metadata is invalid"))?;

    let fields = validate_field_metadata(&descriptors, &grid_kind)?;
    let first = fields
        .first()
        .ok_or_else(|| storage_error("Zarr store contains no legal data variables"))?;
    for field in &fields[1..] {
        if field.shape != first.shape
            || field.dimensions != first.dimensions
            || field.valid_time != first.valid_time
            || field.crs != first.crs
            || field.affine != first.affine
        {
            return Err(storage_error(
                "Zarr data variables do not share one shape, grid, and valid_time",
            ));
        }
    }

    let selected = match variable {
        Some(name) => fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| storage_error("Zarr data variable was not found"))?,
        None if fields.len() == 1 => &fields[0],
        None => {
            return Err(storage_error(
                "Zarr variable selection is ambiguous; provide a variable name",
            ));
        }
    };

    let pixels = checked_product(&selected.shape)?;
    let pixels_u64 = u64::try_from(pixels)
        .map_err(|_| CoreError::ResourceLimit("Zarr field pixel count overflows".into()))?;
    limits.validate_pixels(pixels_u64)?;
    let data_bytes = pixels_u64
        .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
        .ok_or_else(|| CoreError::ResourceLimit("Zarr field byte size overflows".into()))?;
    let coordinate_bytes = selected
        .shape
        .iter()
        .rev()
        .take(2)
        .try_fold(0_u64, |total, size| {
            total.checked_add((*size as u64).saturating_mul(std::mem::size_of::<f64>() as u64))
        })
        .ok_or_else(|| CoreError::ResourceLimit("Zarr coordinate byte size overflows".into()))?;
    let decoded_bytes = data_bytes
        .checked_add(coordinate_bytes)
        .ok_or_else(|| CoreError::ResourceLimit("Zarr decoded size overflows".into()))?;
    if decoded_bytes > limits.max_frame_bytes || decoded_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "Zarr field exceeds configured frame or temporary-file limit".into(),
        ));
    }

    validate_all_chunks_exist(&store_path, &descriptors)?;
    let store = Arc::new(
        FilesystemStore::new(&store_path)
            .map_err(|_| storage_error("Zarr filesystem store could not be initialized"))?,
    );
    let mut arrays = BTreeMap::new();
    for descriptor in descriptors.values() {
        let array = Array::open(store.clone(), &format!("/{}", descriptor.name))
            .map_err(|_| storage_error("Zarr array metadata is invalid or unsupported"))?;
        if array.shape() != descriptor.shape_u64.as_slice()
            || array.attributes() != &descriptor.attributes
        {
            return Err(storage_error("Zarr array metadata conflicts with consolidated metadata"));
        }
        arrays.insert(descriptor.name.clone(), array);
    }

    for coordinate_name in ["time", "crs"] {
        let values = read_scalar_i64(
            arrays
                .get(coordinate_name)
                .ok_or_else(|| storage_error("Zarr scalar coordinate could not be opened"))?,
        )?;
        if values != [0_i64] {
            return Err(storage_error("Zarr scalar coordinate value is invalid"));
        }
    }

    let values = read_array_values::<f32>(
        arrays
            .get(&selected.name)
            .ok_or_else(|| storage_error("selected Zarr data array is missing"))?,
        &selected.shape,
        &descriptors[&selected.name].chunks,
    )?;
    let quality = read_array_values::<u16>(
        arrays
            .get(&selected.quality_name)
            .ok_or_else(|| storage_error("selected Zarr quality array is missing"))?,
        &selected.shape,
        &descriptors[&selected.quality_name].chunks,
    )?;

    let dimensions = &selected.dimensions;
    let geographic = grid_kind == "geographic";
    let (x_name, y_name) = coordinate_names(&selected.shape, dimensions, geographic);
    let x_length =
        *selected.shape.last().ok_or_else(|| storage_error("Zarr field shape is empty"))?;
    let x = read_optional_coordinate(&arrays, &descriptors, x_name, x_length)?;
    let y = if selected.shape.len() >= 2 && y_name != x_name {
        read_optional_coordinate(
            &arrays,
            &descriptors,
            y_name,
            selected.shape[selected.shape.len() - 2],
        )?
    } else {
        Vec::new()
    };

    let field = RadarField {
        name: selected.name.clone(),
        values,
        shape: selected.shape.clone(),
        quality,
        units: selected.units.clone(),
        valid_time: selected.valid_time_text.clone(),
        grid: crate::model::Grid {
            shape: selected.shape.clone(),
            crs: selected.crs.clone(),
            x,
            y,
            affine: selected.affine,
        },
        provenance: selected.provenance.clone(),
    };
    field.validate().map_err(|error| storage_error(error.to_string()))?;
    Ok(field)
}

#[derive(Clone, Debug)]
struct ZarrArrayDescriptor {
    name: String,
    metadata: Value,
    attributes: Map<String, Value>,
    shape_u64: Vec<u64>,
    shape: Vec<usize>,
    chunks: Vec<usize>,
    dtype: String,
}

#[derive(Clone, Debug)]
struct ZarrFieldDescriptor {
    name: String,
    quality_name: String,
    shape: Vec<usize>,
    dimensions: Vec<String>,
    valid_time: DateTime<Utc>,
    valid_time_text: String,
    crs: Option<String>,
    affine: Option<[f64; 6]>,
    units: Option<String>,
    provenance: Vec<String>,
}

fn inspect_input_directory(path: &Path, limits: &Limits) -> CoreResult<(PathBuf, u64)> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| storage_error("Zarr input directory could not be inspected"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(storage_error("Zarr input must be a real directory"));
    }
    let root = fs::canonicalize(path)
        .map_err(|_| storage_error("Zarr input directory could not be resolved"))?;
    let byte_limit = limits.max_artifact_bytes.min(limits.max_temp_bytes);
    let total_bytes = directory_size_limited(&root, &root, byte_limit)?;
    Ok((root, total_bytes))
}

fn directory_size_limited(root: &Path, directory: &Path, limit: u64) -> CoreResult<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(directory)
        .map_err(|_| storage_error("Zarr input directory could not be read"))?
    {
        let entry = entry.map_err(|_| storage_error("Zarr input entry could not be read"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| storage_error("Zarr input entry metadata could not be read"))?;
        if metadata.file_type().is_symlink() {
            return Err(storage_error("Zarr input contains a symbolic link"));
        }
        let canonical = fs::canonicalize(&path)
            .map_err(|_| storage_error("Zarr input entry could not be resolved"))?;
        if !canonical.starts_with(root) {
            return Err(storage_error("Zarr input entry escapes its directory"));
        }
        let size = if metadata.is_dir() {
            directory_size_limited(root, &canonical, limit.saturating_sub(total))?
        } else if metadata.is_file() {
            metadata.len()
        } else {
            return Err(storage_error("Zarr input contains an unsupported filesystem entry"));
        };
        total = total
            .checked_add(size)
            .ok_or_else(|| CoreError::ResourceLimit("Zarr input size overflows".into()))?;
        if total > limit {
            return Err(CoreError::ResourceLimit(
                "Zarr input exceeds the configured artifact or temporary-file limit".into(),
            ));
        }
    }
    Ok(total)
}

fn read_consolidated_metadata(
    root: &Path,
) -> CoreResult<(Map<String, Value>, BTreeMap<String, ZarrArrayDescriptor>)> {
    let consolidated_path = root.join(".zmetadata");
    let consolidated: Value = read_json_file(&consolidated_path)
        .map_err(|_| storage_error("Zarr consolidated metadata is missing or invalid"))?;
    if consolidated.get("zarr_consolidated_format") != Some(&json!(1)) {
        return Err(storage_error("Zarr consolidated metadata version is unsupported"));
    }
    let metadata = consolidated
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| storage_error("Zarr consolidated metadata map is invalid"))?;
    let root_group = metadata
        .get(".zgroup")
        .ok_or_else(|| storage_error("Zarr root group metadata is missing"))?;
    if root_group.get("zarr_format") != Some(&json!(2)) {
        return Err(storage_error("Zarr root group is not version 2"));
    }
    let group_attrs_value =
        metadata.get(".zattrs").ok_or_else(|| storage_error("Zarr root attributes are missing"))?;
    let group_attrs = group_attrs_value
        .as_object()
        .cloned()
        .ok_or_else(|| storage_error("Zarr root attributes are invalid"))?;
    if group_attrs.get("Conventions") != Some(&json!("CF-1.8")) {
        return Err(storage_error("Zarr root conventions are unsupported"));
    }
    require_matching_json(root, ".zgroup", root_group)?;
    require_matching_json(root, ".zattrs", group_attrs_value)?;

    let mut array_names = BTreeSet::new();
    for key in metadata.keys() {
        if key == ".zgroup" || key == ".zattrs" {
            continue;
        }
        let (name, suffix) = key.rsplit_once('/').ok_or_else(|| {
            storage_error("Zarr consolidated metadata contains an unsupported node")
        })?;
        if !valid_array_name(name) || (suffix != ".zarray" && suffix != ".zattrs") {
            return Err(storage_error("Zarr consolidated metadata contains an unsupported node"));
        }
        array_names.insert(name.to_owned());
    }

    let mut arrays = BTreeMap::new();
    for name in array_names {
        let metadata_key = format!("{name}/.zarray");
        let attributes_key = format!("{name}/.zattrs");
        let array_metadata = metadata
            .get(&metadata_key)
            .ok_or_else(|| storage_error("Zarr array metadata is incomplete"))?;
        let attributes_value = metadata
            .get(&attributes_key)
            .ok_or_else(|| storage_error("Zarr array attributes are missing"))?;
        let attributes = attributes_value
            .as_object()
            .cloned()
            .ok_or_else(|| storage_error("Zarr array attributes are invalid"))?;
        require_matching_json(root, &metadata_key, array_metadata)?;
        require_matching_json(root, &attributes_key, attributes_value)?;
        let descriptor = parse_array_descriptor(name.clone(), array_metadata, attributes)?;
        arrays.insert(name, descriptor);
    }
    if arrays.is_empty() {
        return Err(storage_error("Zarr store contains no arrays"));
    }
    Ok((group_attrs, arrays))
}

fn read_json_file(path: &Path) -> Result<Value, serde_json::Error> {
    let bytes = fs::read(path).map_err(|error| serde_json::Error::io(error))?;
    serde_json::from_slice(&bytes)
}

fn require_matching_json(root: &Path, key: &str, expected: &Value) -> CoreResult<()> {
    let local = read_json_file(&root.join(key))
        .map_err(|_| storage_error("Zarr local metadata is missing or invalid"))?;
    if &local != expected {
        return Err(storage_error("Zarr local metadata conflicts with consolidated metadata"));
    }
    Ok(())
}

fn parse_array_descriptor(
    name: String,
    metadata: &Value,
    attributes: Map<String, Value>,
) -> CoreResult<ZarrArrayDescriptor> {
    let object =
        metadata.as_object().ok_or_else(|| storage_error("Zarr array metadata is invalid"))?;
    if object.get("zarr_format") != Some(&json!(2))
        || object.get("order") != Some(&json!("C"))
        || object.get("dimension_separator").is_some_and(|separator| separator != ".")
        || object.get("filters").is_some_and(|filters| !filters.is_null() && filters != &json!([]))
    {
        return Err(storage_error("Zarr array metadata is unsupported"));
    }
    let shape_u64 = json_u64_array(object.get("shape"))?;
    let chunks_u64 = json_u64_array(object.get("chunks"))?;
    if shape_u64.len() != chunks_u64.len()
        || shape_u64.iter().any(|dimension| *dimension == 0)
        || chunks_u64
            .iter()
            .zip(&shape_u64)
            .any(|(chunk, dimension)| *chunk == 0 || chunk > dimension)
    {
        return Err(storage_error("Zarr array shape or chunk metadata is invalid"));
    }
    let shape = shape_u64
        .iter()
        .map(|value| usize::try_from(*value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CoreError::ResourceLimit("Zarr array shape exceeds address space".into()))?;
    let chunks = chunks_u64
        .iter()
        .map(|value| usize::try_from(*value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CoreError::ResourceLimit("Zarr chunk shape exceeds address space".into()))?;
    let dtype = object
        .get("dtype")
        .and_then(Value::as_str)
        .ok_or_else(|| storage_error("Zarr array data type is missing"))?
        .to_owned();
    Ok(ZarrArrayDescriptor {
        name,
        metadata: metadata.clone(),
        attributes,
        shape_u64,
        shape,
        chunks,
        dtype,
    })
}

fn json_u64_array(value: Option<&Value>) -> CoreResult<Vec<u64>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| storage_error("Zarr shape metadata is invalid"))?
        .iter()
        .map(|value| value.as_u64().ok_or_else(|| storage_error("Zarr shape metadata is invalid")))
        .collect()
}

fn required_string(attributes: &Map<String, Value>, key: &str) -> CoreResult<String> {
    attributes
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| storage_error("Zarr required string metadata is missing or invalid"))
}

fn optional_string(attributes: &Map<String, Value>, key: &str) -> CoreResult<Option<String>> {
    match attributes.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(storage_error("Zarr string metadata is invalid")),
    }
}

fn dimensions_attribute(attributes: &Map<String, Value>) -> CoreResult<Vec<String>> {
    serde_json::from_value(
        attributes
            .get("_ARRAY_DIMENSIONS")
            .cloned()
            .ok_or_else(|| storage_error("Zarr array dimensions are missing"))?,
    )
    .map_err(|_| storage_error("Zarr array dimensions are invalid"))
}

fn validate_array_layout(
    descriptor: &ZarrArrayDescriptor,
    dtype: &str,
    shape: &[usize],
    chunks: &[usize],
    compressed: bool,
) -> CoreResult<()> {
    if descriptor.dtype != dtype || descriptor.shape != shape || descriptor.chunks != chunks {
        return Err(storage_error("Zarr array shape, dtype, or chunks are unsupported"));
    }
    let object = descriptor
        .metadata
        .as_object()
        .ok_or_else(|| storage_error("Zarr array metadata is invalid"))?;
    let compressor_matches = if compressed {
        object.get("compressor")
            == Some(&json!({
                "id": "blosc",
                "cname": "lz4",
                "clevel": 5,
                "shuffle": 1,
                "blocksize": 0
            }))
    } else {
        object.get("compressor").is_none_or(Value::is_null)
    };
    if !compressor_matches {
        return Err(storage_error("Zarr array codec is unsupported"));
    }
    Ok(())
}

fn expected_dimensions(shape: &[usize], geographic: bool) -> Vec<String> {
    if shape.len() == 2 && geographic {
        vec!["latitude".into(), "longitude".into()]
    } else if shape.len() == 2 {
        vec!["y".into(), "x".into()]
    } else {
        (0..shape.len()).map(|index| format!("dim_{index}")).collect()
    }
}

fn coordinate_names<'a>(
    shape: &[usize],
    dimensions: &'a [String],
    geographic: bool,
) -> (&'a str, &'a str) {
    if shape.len() == 2 && geographic {
        ("longitude", "latitude")
    } else if shape.len() == 2 {
        ("x", "y")
    } else {
        let x_name = dimensions.last().map(String::as_str).unwrap_or("dim_0");
        let y_name = dimensions
            .get(dimensions.len().saturating_sub(2))
            .map(String::as_str)
            .unwrap_or("dim_0");
        (x_name, y_name)
    }
}

fn validate_compressed_axis(
    descriptor: &ZarrArrayDescriptor,
    name: &str,
    length: usize,
    geographic_standard_name: Option<&str>,
) -> CoreResult<()> {
    validate_array_layout(descriptor, "<f8", &[length], &[length], true)?;
    if dimensions_attribute(&descriptor.attributes)? != [name] {
        return Err(storage_error("Zarr coordinate dimensions are invalid"));
    }
    if let Some(standard_name) = geographic_standard_name
        && (descriptor.attributes.get("standard_name") != Some(&json!(standard_name))
            || descriptor.attributes.get("units")
                != Some(&json!(if standard_name == "longitude" {
                    "degrees_east"
                } else {
                    "degrees_north"
                })))
    {
        return Err(storage_error("Zarr geographic coordinate metadata is invalid"));
    }
    Ok(())
}

fn validate_field_metadata(
    descriptors: &BTreeMap<String, ZarrArrayDescriptor>,
    grid_kind: &str,
) -> CoreResult<Vec<ZarrFieldDescriptor>> {
    let time =
        descriptors.get("time").ok_or_else(|| storage_error("Zarr time coordinate is missing"))?;
    let crs_array =
        descriptors.get("crs").ok_or_else(|| storage_error("Zarr CRS coordinate is missing"))?;
    validate_array_layout(time, "<i8", &[], &[], false)?;
    validate_array_layout(crs_array, "<i8", &[], &[], false)?;
    if time.metadata.get("fill_value") != Some(&Value::Null)
        || crs_array.metadata.get("fill_value") != Some(&Value::Null)
        || dimensions_attribute(&time.attributes)?.len() != 0
        || required_string(&time.attributes, "calendar")? != "proleptic_gregorian"
        || required_string(&time.attributes, "standard_name")? != "time"
        || dimensions_attribute(&crs_array.attributes)?.len() != 0
        || required_string(&crs_array.attributes, "long_name")? != "coordinate reference system"
        || required_string(&crs_array.attributes, "units")? != "1"
    {
        return Err(storage_error("Zarr scalar coordinate metadata is invalid"));
    }
    let crs_reference = required_string(&crs_array.attributes, "spatial_ref")?;

    let mut field_names = Vec::new();
    for descriptor in descriptors.values() {
        if descriptor.name == "time" || descriptor.name == "crs" {
            continue;
        }
        let attrs = &descriptor.attributes;
        if attrs.contains_key("valid_time")
            || attrs.contains_key("ancillary_variables")
            || attrs.contains_key("provenance")
            || attrs.get("long_name").and_then(Value::as_str) == Some(&descriptor.name)
        {
            field_names.push(descriptor.name.clone());
        }
    }
    if field_names.is_empty() {
        return Err(storage_error("Zarr store contains no legal data variables"));
    }

    let mut fields = Vec::with_capacity(field_names.len());
    let mut quality_names = BTreeSet::new();
    for name in &field_names {
        let descriptor = &descriptors[name];
        if descriptor.shape.is_empty() || descriptor.shape.contains(&0) {
            return Err(storage_error("Zarr data variable shape is invalid"));
        }
        checked_product(&descriptor.shape)?;
        validate_array_layout(
            descriptor,
            "<f4",
            &descriptor.shape,
            &default_chunk_shape(&descriptor.shape),
            true,
        )?;
        let attrs = &descriptor.attributes;
        if required_string(attrs, "long_name")? != descriptor.name
            || required_string(attrs, "grid_mapping")? != "crs"
        {
            return Err(storage_error("Zarr data variable metadata is invalid"));
        }
        let dimensions = dimensions_attribute(attrs)?;
        if dimensions != expected_dimensions(&descriptor.shape, grid_kind == "geographic") {
            return Err(storage_error("Zarr data variable dimensions are unsupported"));
        }
        let quality_name = required_string(attrs, "ancillary_variables")?;
        if !valid_array_name(&quality_name) || !quality_names.insert(quality_name.clone()) {
            return Err(storage_error("Zarr quality array reference is invalid"));
        }
        if required_string(attrs, "coordinates")?
            != auxiliary_coordinates(Some(&quality_name), true)
        {
            return Err(storage_error("Zarr data variable coordinates are invalid"));
        }
        let valid_time_text = required_string(attrs, "valid_time")?;
        let valid_time = parse_utc_time(&valid_time_text)
            .map_err(|_| storage_error("Zarr data variable valid_time is invalid"))?;
        let provenance_text = required_string(attrs, "provenance")?;
        let provenance: Vec<String> = serde_json::from_str(&provenance_text)
            .map_err(|_| storage_error("Zarr data variable provenance is invalid"))?;
        let crs = optional_string(attrs, "crs")?;
        if crs.as_deref().unwrap_or("unknown") != crs_reference
            || is_geographic(crs.as_deref()) != (grid_kind == "geographic")
        {
            return Err(storage_error("Zarr data variable CRS conflicts with the grid"));
        }
        let affine = optional_string(attrs, "affine")?
            .map(|value| {
                serde_json::from_str::<[f64; 6]>(&value)
                    .map_err(|_| storage_error("Zarr affine metadata is invalid"))
            })
            .transpose()?;
        if affine.is_some_and(|values| values.iter().any(|value| !value.is_finite())) {
            return Err(storage_error("Zarr affine metadata is non-finite"));
        }
        let units = optional_string(attrs, "units")?;
        fields.push(ZarrFieldDescriptor {
            name: descriptor.name.clone(),
            quality_name,
            shape: descriptor.shape.clone(),
            dimensions,
            valid_time,
            valid_time_text,
            crs,
            affine,
            units,
            provenance,
        });
    }

    let time_units = required_string(&time.attributes, "units")?;
    let time_origin = parse_zarr_time_origin(&time_units)?;
    if fields.iter().any(|field| field.valid_time != time_origin) {
        return Err(storage_error("Zarr valid_time metadata conflicts with the time coordinate"));
    }

    let first = fields
        .first()
        .ok_or_else(|| storage_error("Zarr store contains no legal data variables"))?;
    let geographic = grid_kind == "geographic";
    let (x_name, y_name) = coordinate_names(&first.shape, &first.dimensions, geographic);
    let mut coordinate_names = BTreeSet::new();
    coordinate_names.insert(x_name.to_owned());
    if first.shape.len() >= 2 && y_name != x_name {
        coordinate_names.insert(y_name.to_owned());
    }
    for field in &fields {
        if field.name == "time"
            || field.name == "crs"
            || field.quality_name == field.name
            || coordinate_names.contains(&field.name)
            || coordinate_names.contains(&field.quality_name)
        {
            return Err(storage_error("Zarr field and coordinate names collide"));
        }
        let quality = descriptors
            .get(&field.quality_name)
            .ok_or_else(|| storage_error("Zarr quality array is missing"))?;
        validate_array_layout(
            quality,
            "<u2",
            &field.shape,
            &default_chunk_shape(&field.shape),
            true,
        )?;
        if dimensions_attribute(&quality.attributes)? != field.dimensions
            || required_string(&quality.attributes, "long_name")? != "quality flags"
            || required_string(&quality.attributes, "coordinates")?
                != auxiliary_coordinates(None, true)
            || quality.attributes.get("flag_masks") != Some(&json!(QUALITY_FLAGS))
            || required_string(&quality.attributes, "flag_meanings")? != QUALITY_MEANINGS
        {
            return Err(storage_error("Zarr quality metadata is invalid"));
        }
    }

    let mut recognized = BTreeSet::from(["time".to_owned(), "crs".to_owned()]);
    recognized.extend(field_names);
    recognized.extend(quality_names);
    recognized.extend(coordinate_names.iter().cloned());
    if descriptors.keys().any(|name| !recognized.contains(name)) {
        return Err(storage_error("Zarr store contains an unsupported array"));
    }

    for (axis_name, length, role) in [
        (
            x_name,
            *first.shape.last().ok_or_else(|| storage_error("Zarr field shape is empty"))?,
            "x",
        ),
        (y_name, if first.shape.len() >= 2 { first.shape[first.shape.len() - 2] } else { 0 }, "y"),
    ] {
        if length == 0 || (role == "y" && axis_name == x_name) {
            continue;
        }
        if let Some(axis) = descriptors.get(axis_name) {
            let geographic_name = if geographic {
                Some(if role == "x" { "longitude" } else { "latitude" })
            } else {
                None
            };
            validate_compressed_axis(axis, axis_name, length, geographic_name)?;
        }
    }

    Ok(fields)
}

fn parse_zarr_time_origin(units: &str) -> CoreResult<DateTime<Utc>> {
    let origin = units
        .strip_prefix("days since ")
        .ok_or_else(|| storage_error("Zarr time units are unsupported"))?;
    if origin.contains('T') || origin.ends_with('Z') || origin.contains('+') {
        return Err(storage_error("Zarr time units are unsupported"));
    }
    let (date, time) =
        origin.split_once(' ').ok_or_else(|| storage_error("Zarr time units are invalid"))?;
    let value = format!("{date}T{time}Z");
    parse_utc_time(&value).map_err(|_| storage_error("Zarr time units are invalid"))
}

fn validate_all_chunks_exist(
    root: &Path,
    descriptors: &BTreeMap<String, ZarrArrayDescriptor>,
) -> CoreResult<()> {
    for descriptor in descriptors.values() {
        // Scalar time and CRS arrays use the valid Zarr v2 zero fill and may
        // therefore have no physical chunk. Data, quality, and vector arrays
        // must be physically complete.
        if descriptor.name == "time" || descriptor.name == "crs" {
            continue;
        }
        let grid_shape = descriptor
            .shape
            .iter()
            .zip(&descriptor.chunks)
            .map(|(size, chunk)| size.div_ceil(*chunk))
            .collect::<Vec<_>>();
        let chunk_count = checked_product(&grid_shape)?;
        let directory = root.join(&descriptor.name);
        for number in 0..chunk_count {
            let indices = unflatten(number, &grid_shape);
            let key = if indices.is_empty() {
                "0".to_owned()
            } else {
                indices.iter().map(usize::to_string).collect::<Vec<_>>().join(".")
            };
            let chunk_path = directory.join(&key);
            let metadata = fs::symlink_metadata(&chunk_path).map_err(|_| {
                storage_error(format!(
                    "Zarr coordinate array {} is missing required chunk {}",
                    descriptor.name, key
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(storage_error("Zarr data chunk is not a regular file"));
            }
        }
    }
    Ok(())
}

fn read_array_values<T: ElementOwned + Copy + Default>(
    array: &Array<FilesystemStore>,
    shape: &[usize],
    chunk_shape: &[usize],
) -> CoreResult<Vec<T>> {
    let array_label = format!("{:?}", array.path());
    let element_count = checked_product(shape)?;
    let chunk_grid = shape
        .iter()
        .zip(chunk_shape)
        .map(|(size, chunk)| size.div_ceil(*chunk))
        .collect::<Vec<_>>();
    let chunk_count = checked_product(&chunk_grid)?;
    let chunk_len = checked_product(chunk_shape)?;
    let mut strides = vec![1_usize; shape.len()];
    for index in (0..shape.len().saturating_sub(1)).rev() {
        strides[index] = strides[index + 1]
            .checked_mul(shape[index + 1])
            .ok_or_else(|| CoreError::ResourceLimit("Zarr read stride overflows".into()))?;
    }
    let mut values = vec![T::default(); element_count];
    for chunk_number in 0..chunk_count {
        let indices = unflatten(chunk_number, &chunk_grid);
        let chunk_indices = indices.iter().map(|index| *index as u64).collect::<Vec<_>>();
        let chunk = array
            .retrieve_chunk_if_exists::<Vec<T>>(&chunk_indices)
            .map_err(|_| {
                storage_error(format!("Zarr chunk for {array_label} could not be decoded"))
            })?
            .ok_or_else(|| {
                storage_error(format!("Zarr array {array_label} is missing a required data chunk"))
            })?;
        if chunk.len() != chunk_len {
            return Err(storage_error("Zarr decoded chunk has an invalid element count"));
        }
        for local_number in 0..chunk_len {
            let local = unflatten(local_number, chunk_shape);
            let mut destination = 0_usize;
            let mut inside = true;
            for dimension in 0..shape.len() {
                let global = indices[dimension]
                    .checked_mul(chunk_shape[dimension])
                    .and_then(|start| start.checked_add(local[dimension]))
                    .ok_or_else(|| CoreError::ResourceLimit("Zarr read index overflows".into()))?;
                if global >= shape[dimension] {
                    inside = false;
                    break;
                }
                destination = destination
                    .checked_add(global.checked_mul(strides[dimension]).ok_or_else(|| {
                        CoreError::ResourceLimit("Zarr read offset overflows".into())
                    })?)
                    .ok_or_else(|| CoreError::ResourceLimit("Zarr read offset overflows".into()))?;
            }
            if inside {
                values[destination] = chunk[local_number];
            }
        }
    }
    Ok(values)
}

fn read_scalar_i64(array: &Array<FilesystemStore>) -> CoreResult<Vec<i64>> {
    match array
        .retrieve_chunk_if_exists::<Vec<i64>>(&[])
        .map_err(|_| storage_error("Zarr scalar coordinate chunk could not be decoded"))?
    {
        Some(values) => Ok(values),
        None => array
            .retrieve_chunk::<Vec<i64>>(&[])
            .map_err(|_| storage_error("Zarr scalar coordinate fill value is invalid")),
    }
}

fn read_optional_coordinate(
    arrays: &BTreeMap<String, Array<FilesystemStore>>,
    descriptors: &BTreeMap<String, ZarrArrayDescriptor>,
    name: &str,
    length: usize,
) -> CoreResult<Vec<f64>> {
    let Some(descriptor) = descriptors.get(name) else {
        return Ok(Vec::new());
    };
    if descriptor.shape != [length] {
        return Err(storage_error("Zarr coordinate length does not match the field shape"));
    }
    let array = arrays
        .get(name)
        .ok_or_else(|| storage_error("Zarr coordinate array could not be opened"))?;
    let values = read_array_values::<f64>(array, &[length], &[length])?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(storage_error("Zarr coordinate contains a non-finite value"));
    }
    Ok(values)
}

fn write_fields(fields: &[RadarField], destination: &Path, limits: &Limits) -> CoreResult<PathBuf> {
    if fields.is_empty() {
        return Err(storage_error("Zarr output requires at least one field"));
    }
    validate_array_names(fields)?;
    validate_output_budget(fields, limits)?;

    let destination = destination.to_path_buf();
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|_| storage_error("Zarr output directory could not be prepared"))?;
    let stage = tempfile::Builder::new()
        .prefix(".radiust-zarr-")
        .tempdir_in(parent)
        .map_err(|_| CoreError::Temporary("Zarr staging directory could not be created".into()))?;
    let store_path = stage.path().join("store.zarr");
    fs::create_dir(&store_path)
        .map_err(|_| CoreError::Temporary("Zarr staging store could not be created".into()))?;
    if store_path.to_str().is_none() {
        return Err(storage_error("Zarr output path must be valid UTF-8"));
    }
    let store = Arc::new(
        FilesystemStore::new(&store_path)
            .map_err(|_| storage_error("Zarr filesystem store could not be initialized"))?,
    );

    let first = &fields[0];
    let mut group_attrs = Map::new();
    group_attrs.insert("Conventions".into(), Value::String("CF-1.8".into()));
    group_attrs.insert("radiust_grid_kind".into(), Value::String(grid_kind(first).into()));
    group_attrs.insert(
        "radiust_provenance".into(),
        serde_json::to_value(&first.provenance)
            .map_err(|_| storage_error("Zarr provenance metadata could not be serialized"))?,
    );
    let root = Group::new_with_metadata(
        store.clone(),
        "/",
        GroupMetadataV2::new().with_attributes(group_attrs).into(),
    )
    .map_err(|_| storage_error("Zarr root group could not be created"))?;
    root.store_metadata().map_err(|_| storage_error("Zarr root metadata could not be written"))?;

    let dimensions = dimension_names(first);
    write_coordinate_arrays(store.clone(), first, &dimensions)?;

    for field in fields {
        let data_chunks = default_chunk_shape(&field.shape);
        let quality_name = if fields.len() == 1 {
            "quality".to_owned()
        } else {
            format!("{}_quality", field.name)
        };
        let quality_attrs = quality_attributes(&dimensions, true);
        let quality_array = create_array::<u16>(
            store.clone(),
            &format!("/{quality_name}"),
            &field.shape,
            &data_chunks,
            "<u2",
            FillValueMetadata::Null,
            true,
            quality_attrs,
        )?;
        write_array_chunks(&quality_array, &field.shape, &data_chunks, &field.quality, 0_u16)?;

        let mut data_attrs = data_attributes(field, &dimensions, &quality_name)?;
        data_attrs.insert("grid_mapping".into(), Value::String("crs".into()));
        let data_array = create_array::<f32>(
            store.clone(),
            &format!("/{}", field.name),
            &field.shape,
            &data_chunks,
            "<f4",
            FillValueMetadata::from(f32::NAN),
            true,
            data_attrs,
        )?;
        write_array_chunks(&data_array, &field.shape, &data_chunks, &field.values, f32::NAN)?;
    }

    consolidate_metadata(&store, &store_path)?;
    let output_bytes = directory_size(&store_path)?;
    if output_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "Zarr output exceeds the configured temporary-file limit".into(),
        ));
    }

    publish_directory(&store_path, &destination)?;
    Ok(destination)
}

fn validate_array_names(fields: &[RadarField]) -> CoreResult<()> {
    let geographic = is_geographic(fields[0].grid.crs.as_deref());
    let reserved = if geographic {
        ["time", "crs", "latitude", "longitude"]
    } else {
        ["time", "crs", "x", "y"]
    };
    let mut names = BTreeSet::new();
    for field in fields {
        if !valid_array_name(&field.name) || reserved.contains(&field.name.as_str()) {
            return Err(storage_error("Zarr field name is invalid or reserved"));
        }
        if !names.insert(field.name.clone()) {
            return Err(storage_error("Zarr field names must be unique"));
        }
        let quality_name = if fields.len() == 1 {
            "quality".to_owned()
        } else {
            format!("{}_quality", field.name)
        };
        if !names.insert(quality_name) {
            return Err(storage_error("Zarr field and quality array names collide"));
        }
    }
    for coordinate in ["time", "crs"] {
        if !names.insert(coordinate.to_owned()) {
            return Err(storage_error("Zarr field collides with a coordinate array"));
        }
    }
    for coordinate in if geographic { ["latitude", "longitude"] } else { ["y", "x"] } {
        if !names.insert(coordinate.to_owned()) {
            return Err(storage_error("Zarr field collides with a coordinate array"));
        }
    }
    Ok(())
}

fn valid_array_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn validate_output_budget(fields: &[RadarField], limits: &Limits) -> CoreResult<()> {
    let mut total_bytes = 0_u64;
    for field in fields {
        let pixels =
            field.shape.iter().try_fold(1_u64, |total, size| total.checked_mul(*size as u64));
        let pixels =
            pixels.ok_or_else(|| CoreError::ResourceLimit("Zarr field shape overflows".into()))?;
        limits.validate_pixels(pixels)?;
        let bytes = pixels
            .checked_mul((std::mem::size_of::<f32>() + std::mem::size_of::<u16>()) as u64)
            .ok_or_else(|| CoreError::ResourceLimit("Zarr field byte size overflows".into()))?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| CoreError::ResourceLimit("Zarr dataset byte size overflows".into()))?;
    }
    for coordinate_len in [fields[0].grid.x.len(), fields[0].grid.y.len()] {
        total_bytes = total_bytes
            .checked_add((coordinate_len as u64).saturating_mul(std::mem::size_of::<f64>() as u64))
            .ok_or_else(|| CoreError::ResourceLimit("Zarr coordinate size overflows".into()))?;
    }
    if total_bytes > limits.max_frame_bytes || total_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "Zarr output exceeds configured frame or temporary-file limit".into(),
        ));
    }
    Ok(())
}

fn dimension_names(field: &RadarField) -> Vec<String> {
    if field.shape.len() == 2 && is_geographic(field.grid.crs.as_deref()) {
        vec!["latitude".into(), "longitude".into()]
    } else if field.shape.len() == 2 {
        vec!["y".into(), "x".into()]
    } else {
        (0..field.shape.len()).map(|index| format!("dim_{index}")).collect()
    }
}

fn grid_kind(field: &RadarField) -> &'static str {
    if is_geographic(field.grid.crs.as_deref()) { "geographic" } else { "cartesian" }
}

fn is_geographic(crs: Option<&str>) -> bool {
    crs.is_some_and(|crs| matches!(crs.to_ascii_uppercase().as_str(), "EPSG:4326" | "EPSG:3821"))
}

fn write_coordinate_arrays(
    store: Arc<FilesystemStore>,
    field: &RadarField,
    dimensions: &[String],
) -> CoreResult<()> {
    let geographic = is_geographic(field.grid.crs.as_deref());
    let (x_name, y_name) = if dimensions.len() == 2 && geographic {
        ("longitude", "latitude")
    } else if dimensions.len() == 2 {
        ("x", "y")
    } else {
        let x_name = dimensions.last().map(String::as_str).unwrap_or("dim_0");
        let y_name = dimensions
            .get(dimensions.len().saturating_sub(2))
            .map(String::as_str)
            .unwrap_or("dim_0");
        (x_name, y_name)
    };

    if !field.grid.x.is_empty() {
        let mut attrs = dimension_attributes(x_name);
        if geographic {
            attrs.insert("standard_name".into(), Value::String("longitude".into()));
            attrs.insert("units".into(), Value::String("degrees_east".into()));
        }
        let array = create_array::<f64>(
            store.clone(),
            &format!("/{x_name}"),
            &[field.grid.x.len()],
            &[field.grid.x.len()],
            "<f8",
            FillValueMetadata::from(f64::NAN),
            true,
            attrs,
        )?;
        write_array_chunks(
            &array,
            &[field.grid.x.len()],
            &[field.grid.x.len()],
            &field.grid.x,
            f64::NAN,
        )?;
    }
    if !field.grid.y.is_empty() {
        let mut attrs = dimension_attributes(y_name);
        if geographic {
            attrs.insert("standard_name".into(), Value::String("latitude".into()));
            attrs.insert("units".into(), Value::String("degrees_north".into()));
        }
        let array = create_array::<f64>(
            store.clone(),
            &format!("/{y_name}"),
            &[field.grid.y.len()],
            &[field.grid.y.len()],
            "<f8",
            FillValueMetadata::from(f64::NAN),
            true,
            attrs,
        )?;
        write_array_chunks(
            &array,
            &[field.grid.y.len()],
            &[field.grid.y.len()],
            &field.grid.y,
            f64::NAN,
        )?;
    }

    let time = parse_utc_time(&field.valid_time)
        .map_err(|_| storage_error("Zarr field valid_time is invalid"))?;
    let time_units = format!(
        "days since {}",
        time.to_rfc3339_opts(SecondsFormat::AutoSi, true).replace('T', " ").trim_end_matches('Z')
    );
    let time_attrs = Map::from_iter([
        ("_ARRAY_DIMENSIONS".into(), json!([])),
        ("calendar".into(), json!("proleptic_gregorian")),
        ("standard_name".into(), json!("time")),
        ("units".into(), json!(time_units)),
    ]);
    let time_array = create_array::<i64>(
        store.clone(),
        "/time",
        &[],
        &[],
        "<i8",
        FillValueMetadata::Null,
        false,
        time_attrs,
    )?;
    time_array
        .store_chunk(&[], &[0_i64])
        .map_err(|_| storage_error("Zarr time coordinate could not be written"))?;

    let crs_attrs = crs_attributes(field.grid.crs.as_deref());
    let crs_array = create_array::<i64>(
        store,
        "/crs",
        &[],
        &[],
        "<i8",
        FillValueMetadata::Null,
        false,
        crs_attrs,
    )?;
    crs_array
        .store_chunk(&[], &[0_i64])
        .map_err(|_| storage_error("Zarr CRS coordinate could not be written"))
}

fn crs_attributes(crs: Option<&str>) -> Map<String, Value> {
    let mut attrs = Map::from_iter([
        ("_ARRAY_DIMENSIONS".into(), json!([])),
        ("long_name".into(), json!("coordinate reference system")),
        ("spatial_ref".into(), json!(crs.unwrap_or("unknown"))),
        ("units".into(), json!("1")),
    ]);
    match crs.map(str::to_ascii_uppercase).as_deref() {
        Some("EPSG:4326") => {
            attrs.insert("grid_mapping_name".into(), json!("latitude_longitude"));
            attrs.insert("semi_major_axis".into(), json!(6_378_137.0));
            attrs.insert("inverse_flattening".into(), json!(298.257_223_563));
        }
        Some("EPSG:3821") => {
            attrs.insert("grid_mapping_name".into(), json!("latitude_longitude"));
            attrs.insert("geographic_crs_name".into(), json!("TWD67"));
            attrs.insert("semi_major_axis".into(), json!(6_378_160.0));
            attrs.insert("inverse_flattening".into(), json!(298.25));
        }
        _ => {}
    }
    attrs
}

fn dimension_attributes(dimension: &str) -> Map<String, Value> {
    Map::from_iter([("_ARRAY_DIMENSIONS".into(), json!([dimension]))])
}

fn quality_attributes(dimensions: &[String], has_crs: bool) -> Map<String, Value> {
    let mut attrs = Map::from_iter([
        (
            "_ARRAY_DIMENSIONS".into(),
            Value::Array(dimensions.iter().cloned().map(Value::String).collect()),
        ),
        ("flag_masks".into(), serde_json::to_value(QUALITY_FLAGS).unwrap_or_else(|_| json!([]))),
        ("flag_meanings".into(), json!(QUALITY_MEANINGS)),
        ("long_name".into(), json!("quality flags")),
    ]);
    let coordinates = auxiliary_coordinates(None, has_crs);
    if !coordinates.is_empty() {
        attrs.insert("coordinates".into(), json!(coordinates));
    }
    attrs
}

fn data_attributes(
    field: &RadarField,
    dimensions: &[String],
    quality_name: &str,
) -> CoreResult<Map<String, Value>> {
    let mut attrs = Map::from_iter([
        (
            "_ARRAY_DIMENSIONS".into(),
            Value::Array(dimensions.iter().cloned().map(Value::String).collect()),
        ),
        ("long_name".into(), json!(field.name)),
        ("valid_time".into(), json!(field.valid_time)),
        (
            "provenance".into(),
            Value::String(
                serde_json::to_string(&field.provenance)
                    .map_err(|_| storage_error("Zarr field provenance could not be serialized"))?,
            ),
        ),
        ("ancillary_variables".into(), json!(quality_name)),
    ]);
    if let Some(units) = field.units.as_deref() {
        attrs.insert("units".into(), json!(units));
    }
    if let Some(crs) = field.grid.crs.as_deref() {
        attrs.insert("crs".into(), json!(crs));
        attrs.insert("grid_mapping".into(), json!("crs"));
    }
    if let Some(affine) = field.grid.affine {
        attrs.insert(
            "affine".into(),
            Value::String(
                serde_json::to_string(&affine)
                    .map_err(|_| storage_error("Zarr affine metadata could not be serialized"))?,
            ),
        );
    }
    let coordinates = auxiliary_coordinates(Some(quality_name), true);
    if !coordinates.is_empty() {
        attrs.insert("coordinates".into(), json!(coordinates));
    }
    Ok(attrs)
}

fn auxiliary_coordinates(quality: Option<&str>, has_crs: bool) -> String {
    let mut coordinates = BTreeSet::new();
    if has_crs {
        coordinates.insert("crs");
    }
    coordinates.insert("time");
    if let Some(quality) = quality {
        coordinates.insert(quality);
    }
    coordinates.into_iter().collect::<Vec<_>>().join(" ")
}

fn create_array<T: Element>(
    store: Arc<FilesystemStore>,
    path: &str,
    shape: &[usize],
    chunk_shape: &[usize],
    dtype: &str,
    fill_value: FillValueMetadata,
    compressed: bool,
    attributes: Map<String, Value>,
) -> CoreResult<Array<FilesystemStore>> {
    let shape_u64 = shape
        .iter()
        .map(|value| u64::try_from(*value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CoreError::ResourceLimit("Zarr array shape exceeds u64".into()))?;
    if shape.len() != chunk_shape.len()
        || shape
            .iter()
            .zip(chunk_shape)
            .any(|(size, chunk)| *size == 0 || *chunk == 0 || chunk > size)
    {
        return Err(storage_error("Zarr chunk shape does not match the array shape"));
    }
    let chunks = chunk_shape
        .iter()
        .map(|size| {
            NonZeroU64::new(*size as u64)
                .ok_or_else(|| storage_error("Zarr chunk dimensions must be positive"))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let compressor = compressed
        .then(|| {
            serde_json::from_value(json!({
                "id": "blosc",
                "cname": "lz4",
                "clevel": 5,
                "shuffle": 1,
                "blocksize": 0
            }))
            .map_err(|_| storage_error("Zarr Blosc metadata could not be initialized"))
        })
        .transpose()?;
    let metadata =
        ArrayMetadataV2::new(shape_u64, chunks, dtype.into(), fill_value, compressor, None)
            .with_attributes(attributes);
    let array = Array::new_with_metadata(store, path, metadata.into())
        .map_err(|_| storage_error("Zarr array metadata is invalid or unsupported"))?;
    array
        .store_metadata_opt(&ArrayMetadataOptions::default().with_include_zarrs_metadata(false))
        .map_err(|_| storage_error("Zarr array metadata could not be written"))?;
    Ok(array)
}

fn write_array_chunks<T: Element + Copy>(
    array: &Array<FilesystemStore>,
    shape: &[usize],
    chunk_shape: &[usize],
    values: &[T],
    fill_value: T,
) -> CoreResult<()> {
    if shape.len() != chunk_shape.len() || checked_product(shape)? != values.len() {
        return Err(storage_error("Zarr values do not match their declared shape"));
    }
    let grid_shape = shape
        .iter()
        .zip(chunk_shape)
        .map(|(size, chunk)| size.div_ceil(*chunk))
        .collect::<Vec<_>>();
    let chunk_count = checked_product(&grid_shape)?;
    let chunk_len = checked_product(&chunk_shape)?;
    let codec_options = CodecOptions::default().with_store_empty_chunks(true);
    let mut strides = vec![1_usize; shape.len()];
    for index in (0..shape.len().saturating_sub(1)).rev() {
        strides[index] = strides[index + 1]
            .checked_mul(shape[index + 1])
            .ok_or_else(|| CoreError::ResourceLimit("Zarr array stride overflows".into()))?;
    }
    for chunk_number in 0..chunk_count {
        let indices = unflatten(chunk_number, &grid_shape);
        let mut data = vec![fill_value; chunk_len];
        for local_number in 0..chunk_len {
            let local = unflatten(local_number, &chunk_shape);
            let mut source_index = 0_usize;
            let mut inside = true;
            for dimension in 0..shape.len() {
                let global = indices[dimension]
                    .checked_mul(chunk_shape[dimension])
                    .and_then(|start| start.checked_add(local[dimension]))
                    .ok_or_else(|| CoreError::ResourceLimit("Zarr chunk index overflows".into()))?;
                if global >= shape[dimension] {
                    inside = false;
                    break;
                }
                source_index = source_index
                    .checked_add(global.checked_mul(strides[dimension]).ok_or_else(|| {
                        CoreError::ResourceLimit("Zarr source offset overflows".into())
                    })?)
                    .ok_or_else(|| {
                        CoreError::ResourceLimit("Zarr source offset overflows".into())
                    })?;
            }
            if inside {
                data[local_number] = *values
                    .get(source_index)
                    .ok_or_else(|| storage_error("Zarr field buffer does not match its shape"))?;
            }
        }
        let chunk_indices = indices.iter().map(|index| *index as u64).collect::<Vec<_>>();
        array
            .store_chunk_opt(&chunk_indices, data.as_slice(), &codec_options)
            .map_err(|_| storage_error("Zarr data chunk could not be encoded or written"))?;
    }
    Ok(())
}

fn default_chunk_shape(shape: &[usize]) -> Vec<usize> {
    shape.iter().map(|size| (*size).min(CHUNK_EDGE)).collect()
}

fn checked_product(values: &[usize]) -> CoreResult<usize> {
    values.iter().try_fold(1_usize, |total, value| {
        total
            .checked_mul(*value)
            .ok_or_else(|| CoreError::ResourceLimit("Zarr chunk size overflows".into()))
    })
}

fn unflatten(mut linear: usize, shape: &[usize]) -> Vec<usize> {
    let mut indices = vec![0; shape.len()];
    for dimension in (0..shape.len()).rev() {
        indices[dimension] = linear % shape[dimension];
        linear /= shape[dimension];
    }
    indices
}

fn consolidate_metadata(store: &FilesystemStore, store_path: &Path) -> CoreResult<()> {
    let keys = store.list().map_err(|_| storage_error("Zarr metadata keys could not be listed"))?;
    let mut metadata = BTreeMap::new();
    for key in keys {
        let name = key.as_str();
        if !(name.ends_with(".zgroup") || name.ends_with(".zarray") || name.ends_with(".zattrs")) {
            continue;
        }
        let bytes = store
            .get(&key)
            .map_err(|_| storage_error("Zarr metadata could not be read for consolidation"))?
            .ok_or_else(|| storage_error("Zarr metadata disappeared during consolidation"))?;
        let mut value: Value = serde_json::from_slice(bytes.as_ref())
            .map_err(|_| storage_error("Zarr metadata JSON is invalid"))?;
        if name.ends_with(".zarray") {
            let object = value
                .as_object_mut()
                .ok_or_else(|| storage_error("Zarr array metadata must be an object"))?;
            object.remove("node_type");
            if object.get("dimension_separator") == Some(&json!(".")) {
                object.remove("dimension_separator");
            }
            let normalized = serde_json::to_vec_pretty(&value)
                .map_err(|_| storage_error("Zarr array metadata could not be normalized"))?;
            fs::write(store.key_to_fspath(&key), normalized)
                .map_err(|_| storage_error("Zarr array metadata could not be normalized"))?;
        }
        metadata.insert(name.to_owned(), value);
    }
    if !metadata.contains_key(".zgroup") || !metadata.contains_key(".zattrs") {
        return Err(storage_error("Zarr root metadata is incomplete"));
    }
    let consolidated = json!({
        "zarr_consolidated_format": 1,
        "metadata": metadata,
    });
    let bytes = serde_json::to_vec_pretty(&consolidated)
        .map_err(|_| storage_error("Zarr consolidated metadata could not be serialized"))?;
    fs::write(store_path.join(".zmetadata"), bytes)
        .map_err(|_| storage_error("Zarr consolidated metadata could not be written"))
}

fn directory_size(path: &Path) -> CoreResult<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path)
        .map_err(|_| storage_error("Zarr output directory could not be inspected"))?
    {
        let entry = entry.map_err(|_| storage_error("Zarr output entry could not be inspected"))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| storage_error("Zarr output entry metadata could not be read"))?;
        if metadata.file_type().is_symlink() {
            return Err(storage_error("Zarr output contains an unexpected symbolic link"));
        }
        let size = if metadata.is_dir() { directory_size(&entry.path())? } else { metadata.len() };
        total = total
            .checked_add(size)
            .ok_or_else(|| CoreError::ResourceLimit("Zarr output size overflows".into()))?;
    }
    Ok(total)
}

fn publish_directory(staged: &Path, destination: &Path) -> CoreResult<()> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let existing = match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(storage_error("Zarr destination exists and is not a safe directory"));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(storage_error("Zarr destination could not be inspected")),
    };
    if !existing {
        return fs::rename(staged, destination)
            .map_err(|_| storage_error("Zarr directory could not be atomically published"));
    }

    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| storage_error("Zarr destination name must be valid UTF-8"))?;
    let backup = parent.join(format!(".{name}.radiust-backup-{}", uuid::Uuid::new_v4()));
    fs::rename(destination, &backup)
        .map_err(|_| storage_error("existing Zarr output could not be staged for replacement"))?;
    if fs::rename(staged, destination).is_err() {
        let restored = fs::rename(&backup, destination).is_ok();
        return Err(storage_error(if restored {
            "new Zarr output could not be published; previous output was restored"
        } else {
            "new Zarr output could not be published and previous output needs recovery"
        }));
    }
    fs::remove_dir_all(backup).map_err(|_| {
        storage_error("Zarr output was published but its previous generation could not be removed")
    })
}
