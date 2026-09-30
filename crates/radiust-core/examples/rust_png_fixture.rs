use radiust_core::model::{Grid, RadarField};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .ok_or("pass the destination PNG path")?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let field = RadarField {
        name: "reflectivity".into(),
        values: vec![0.0, 1.0, 2.0, f32::NAN],
        shape: vec![2, 2],
        quality: vec![0, 0, 1, 0],
        units: Some("dBZ".into()),
        valid_time: "2026-09-26T00:00:00Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![0.0, 1.0],
            y: vec![-1.0, 1.0],
            affine: None,
        },
        provenance: vec![
            "source=fixture".into(),
            "product=composite".into(),
            "station=east".into(),
        ],
    };
    radiust_core::output::png::write_png(&field, output, &serde_json::json!({}))?;
    Ok(())
}
