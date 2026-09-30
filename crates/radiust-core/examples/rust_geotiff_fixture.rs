use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarField};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or("pass the destination TIFF filename")?;
    let projected_y = [0.0_f64, 1_000_000.0, 2_000_000.0];
    let latitude = projected_y.map(|y| (y / 6_378_137.0).sinh().atan().to_degrees()).to_vec();
    let mut field = RadarField {
        name: "reflectivity".into(),
        values: vec![12.5, f32::NAN, -3.0, 0.25, 1.5, 2.5],
        shape: vec![3, 2],
        quality: vec![0, 1, 4, 0, 2, 3],
        units: Some("dBZ".into()),
        valid_time: "2026-09-26T01:02:03.123456789Z".into(),
        grid: Grid {
            shape: vec![3, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.5],
            y: latitude,
            affine: None,
        },
        provenance: vec!["source=fixture".into()],
    };
    radiust_core::output::geotiff::write_field(&field, &output, &Limits::default())?;

    field.grid.crs = Some("EPSG:3821".into());
    field.grid.y = vec![23.0, 23.5, 24.0];
    let stem = output
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("destination filename must be valid UTF-8")?;
    let twd67_output = output.with_file_name(format!("{stem}_twd67.tif"));
    radiust_core::output::geotiff::write_field(&field, twd67_output, &Limits::default())?;
    Ok(())
}
