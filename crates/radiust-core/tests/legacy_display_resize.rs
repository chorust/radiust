use radiust_core::legacy_display::apply_for_source;
use radiust_core::limits::Limits;

const ID_SOURCE: &[u8] = include_bytes!("../../../tests/fixtures/legacy-display/id/source.png");
const ID_OLD_GRAY: &[u8] = include_bytes!("../../../tests/fixtures/legacy-display/id/old-gray.png");

#[test]
fn id_legacy_display_matches_the_pinned_pillow_bicubic_output_pixel_for_pixel() {
    let source = image::load_from_memory(ID_SOURCE).unwrap().to_rgba8();
    let expected = image::load_from_memory(ID_OLD_GRAY).unwrap().to_luma8();
    let preview = apply_for_source(
        "id",
        "composite",
        None,
        "PNG",
        source.width(),
        source.height(),
        source.as_raw(),
        &Limits::default(),
    )
    .unwrap();

    assert!(preview.applied);
    assert_eq!((preview.width, preview.height), expected.dimensions());
    for (actual, expected) in preview.rgba.chunks_exact(4).zip(expected.as_raw()) {
        assert_eq!(actual, &[*expected, *expected, *expected, 255]);
    }
}
