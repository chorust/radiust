use radiust_core::limits::Limits;
use radiust_core::model::{Grid, RadarField};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or("pass the destination Zarr directory")?;
    let field = RadarField {
        name: "reflectivity".into(),
        values: vec![12.5, f32::NAN, -3.0, 0.25],
        shape: vec![2, 2],
        quality: vec![0, 1, 4, 0],
        units: Some("dBZ".into()),
        valid_time: "2026-09-26T01:02:03.123456789Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![120.0, 120.5],
            y: vec![23.0, 23.5],
            affine: Some([119.75, 0.5, 0.0, 23.75, 0.0, -0.5]),
        },
        provenance: vec!["source=fixture".into()],
    };
    radiust_core::output::zarr::write_field(&field, output, &Limits::default())?;
    Ok(())
}
