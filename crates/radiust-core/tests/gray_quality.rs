use radiust_core::gray::{self, GrayQualityPreview};
use radiust_core::limits::Limits;
use radiust_core::raster::{
    QUALITY_INTERPOLATED, QUALITY_MISSING, QUALITY_OUTSIDE_COVERAGE, QUALITY_RECOVERED,
    QUALITY_SOURCE_ANNOTATION, QUALITY_UNKNOWN_COLOR,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn result(path_id: &str) -> (GrayQualityPreview, Vec<u8>) {
    result_with_pixel(path_id, None)
}

fn result_with_pixel(
    path_id: &str,
    mutation: Option<(u32, u32, [u8; 4])>,
) -> (GrayQualityPreview, Vec<u8>) {
    let root = repo_root();
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("tests/fixtures/legacy-display/manifest.json")).unwrap(),
    )
    .unwrap();
    let entry = manifest["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path_id"] == path_id)
        .unwrap();
    let input_path = root
        .join("tests/fixtures/legacy-display")
        .join(entry["inputs"][0]["path"].as_str().unwrap());
    let bytes = std::fs::read(&input_path).unwrap();
    let mut image = image::load_from_memory(&bytes).unwrap().to_rgba8();
    let (width, height) = image.dimensions();
    if let Some((x, y, pixel)) = mutation {
        image.put_pixel(x, y, image::Rgba(pixel));
    }
    let source = entry["source"].as_str().unwrap();
    let product = entry["product"].as_str().unwrap();
    let station = entry["station"].as_str();
    let format = image::ImageFormat::from_path(&input_path).unwrap();
    let output = gray::apply_for_source_with_quality(
        source,
        product,
        station,
        match format {
            image::ImageFormat::Png => "PNG",
            image::ImageFormat::Gif => "GIF",
            image::ImageFormat::WebP => "WEBP",
            _ => panic!("unexpected historical fixture format {format:?}"),
        },
        width,
        height,
        image.as_raw(),
        &Limits::default(),
    )
    .unwrap();
    (output, image.into_raw())
}

#[test]
fn alpha_masks_are_captured_before_legacy_luminance_forces_opaque_output() {
    let (output, source) = result("fr/composite");
    assert!(output.preview.applied);
    assert!(output.preview.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
    assert_eq!(output.original_alpha.len(), output.quality.len());

    let transparent = output.original_alpha.iter().position(|alpha| *alpha == 0).unwrap();
    assert_ne!(output.quality[transparent] & QUALITY_MISSING, 0);
    assert_ne!(output.origin_quality[transparent] & QUALITY_MISSING, 0);

    let partial = output.original_alpha.iter().position(|alpha| (1..255).contains(alpha)).unwrap();
    assert_ne!(output.quality[partial] & QUALITY_SOURCE_ANNOTATION, 0);

    if let Some(opaque_black) =
        source.chunks_exact(4).position(|pixel| pixel[..3] == [0, 0, 0] && pixel[3] == 255)
    {
        assert_eq!(output.preview.rgba[opaque_black * 4], 0);
        assert_eq!(output.quality[opaque_black] & QUALITY_MISSING, 0);
    }
}

#[test]
fn disk_mask_palette_unknown_and_inpaint_keep_origin_and_current_quality_distinct() {
    let (output, _) = result("au/composite");
    assert!(output.preview.applied);
    assert_eq!(output.quality.len(), 461 * 461);
    assert!(output.quality.iter().any(|quality| quality & QUALITY_OUTSIDE_COVERAGE != 0));
    assert!(output.quality.iter().any(|quality| quality & QUALITY_INTERPOLATED != 0));
    assert!(output.origin_quality.iter().any(|quality| quality & QUALITY_UNKNOWN_COLOR != 0));

    assert!(output.quality.iter().any(|quality| quality & QUALITY_RECOVERED != 0));
    assert!(
        output
            .quality
            .iter()
            .zip(&output.origin_quality)
            .any(|(quality, origin)| quality & QUALITY_RECOVERED != 0
                && origin & QUALITY_UNKNOWN_COLOR != 0)
    );
}

#[test]
fn letterbox_and_zero_invalid_preserve_unresolved_reasons_instead_of_claiming_recovery() {
    let x = 481_u32;
    let y = 539_u32;
    let (output, _) = result_with_pixel("es/composite", Some((x, y, [255, 0, 255, 255])));
    assert!(output.preview.applied);
    let mutated_index = y as usize * 962 + x as usize;
    assert_ne!(output.quality[mutated_index] & QUALITY_UNKNOWN_COLOR, 0);
    assert!(output.quality.iter().all(|quality| quality & QUALITY_RECOVERED == 0));

    let (letterbox, _) = result("th/composite/kkn240Loop");
    assert!(letterbox.origin_quality.iter().any(|quality| quality & QUALITY_OUTSIDE_COVERAGE != 0));
    assert!(letterbox.quality.iter().any(|quality| quality & QUALITY_OUTSIDE_COVERAGE != 0));
}
