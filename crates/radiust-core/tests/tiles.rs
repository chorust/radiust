use image::{Rgba, RgbaImage};
use radiust_core::tiles::{Palette, Tile, crop_lossless, mosaic};

#[test]
fn palette_identity_keeps_same_index_from_different_palettes_distinct() {
    let first = Palette::new("first", vec![[1, 2, 3, 255]]).expect("palette");
    let second = Palette::new("second", vec![[9, 8, 7, 255]]).expect("palette");

    assert_ne!(first.color(0), second.color(0));
    assert_ne!(first.id, second.id);
}

#[test]
fn mosaic_preserves_rgba_without_resize_or_quantization() {
    let mut left = RgbaImage::new(2, 2);
    left.put_pixel(0, 0, Rgba([1, 2, 3, 4]));
    let mut right = RgbaImage::new(2, 2);
    right.put_pixel(1, 1, Rgba([9, 8, 7, 6]));

    let output = mosaic(&[Tile::new(0, 0, left), Tile::new(1, 0, right)], 2, 1).expect("mosaic");

    assert_eq!(output.dimensions(), (4, 2));
    assert_eq!(*output.get_pixel(0, 0), Rgba([1, 2, 3, 4]));
    assert_eq!(*output.get_pixel(3, 1), Rgba([9, 8, 7, 6]));
}

#[test]
fn missing_tiles_fail_and_crop_is_exact() {
    let tile = Tile::new(0, 0, RgbaImage::new(2, 2));
    assert!(mosaic(&[tile], 2, 1).is_err());

    let mut image = RgbaImage::new(3, 2);
    image.put_pixel(1, 0, Rgba([5, 4, 3, 2]));
    let cropped = crop_lossless(&image, 1, 0, 1, 1).expect("crop");
    assert_eq!(cropped.dimensions(), (1, 1));
    assert_eq!(*cropped.get_pixel(0, 0), Rgba([5, 4, 3, 2]));
}
