use super::thumbnail_rgb;
use image::{DynamicImage, ImageBuffer, Rgb, RgbImage, Rgba};

fn reference(image: DynamicImage) -> RgbImage {
    image.thumbnail(1024, 1024).into_rgb8()
}

fn jpeg(image: &RgbImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
        .encode_image(image)
        .unwrap();
    bytes
}

fn compare(image: DynamicImage, check_jpeg: bool) {
    let label = (image.color(), image.width(), image.height());
    let expected = reference(image.clone());
    let actual = thumbnail_rgb(image);
    assert_eq!(actual.dimensions(), expected.dimensions(), "{label:?}");
    assert_eq!(actual.as_raw(), expected.as_raw(), "{label:?}");
    if check_jpeg {
        assert_eq!(jpeg(&actual), jpeg(&expected), "JPEG {label:?}");
    }
}

fn pattern(width: u32, height: u32, rgba: bool, seed: u32) -> DynamicImage {
    let mut state = seed;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as u8
    };
    if rgba {
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
            let alpha = match (x + y) % 4 {
                0 => 0,
                1 => 1,
                2 => 255,
                _ => next(),
            };
            Rgba([next(), next(), next(), alpha])
        }))
    } else {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |_, _| {
            Rgb([next(), next(), next()])
        }))
    }
}

#[test]
fn thumbnail_identity_exhaustive_channel_values() {
    // Every value in every channel, with every RGB value paired with every
    // alpha value, including hidden colors in fully transparent pixels.
    let rgba = ImageBuffer::from_fn(1024, 64, |x, y| {
        let index = y * 1024 + x;
        let value = index as u8;
        Rgba([
            value,
            value.wrapping_add(85),
            255 - value,
            (index >> 8) as u8,
        ])
    });
    compare(DynamicImage::ImageRgba8(rgba), true);
    let rgb = ImageBuffer::from_fn(64, 1024, |x, y| {
        let value = (y * 64 + x) as u8;
        Rgb([value, value.wrapping_add(85), 255 - value])
    });
    compare(DynamicImage::ImageRgb8(rgb), true);
}

#[test]
fn thumbnail_identity_boundary_patterns_and_random_dimensions() {
    let edges = [1, 2, 17, 1023, 1024, 1025];
    for width in edges {
        for height in edges {
            for rgba in [false, true] {
                compare(pattern(width, height, rgba, width * 7919 + height), false);
            }
        }
    }
    let mut state = 0x75a4_1234u32;
    for index in 0..24 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        let short = state % 1024 + 1;
        let (width, height) = match index % 3 {
            0 => (1024, short),
            1 => (short, 1024),
            _ => (short, state.rotate_left(7) % 1300 + 1),
        };
        compare(pattern(width, height, index % 2 == 0, state), index < 6);
    }
}

#[test]
fn thumbnail_identity_jpeg_identity_and_fallback() {
    for (width, height) in [
        (1024, 1),
        (1, 1024),
        (1024, 1023),
        (1024, 1024),
        (1023, 1025),
        (1, 1),
    ] {
        for rgba in [false, true] {
            compare(pattern(width, height, rgba, 0x1234_5678), true);
        }
    }
}

#[test]
fn thumbnail_identity_trailing_storage_matches_reference() {
    for extra in [1, 3, 17] {
        let rgb = RgbImage::from_raw(1024, 1, vec![73; 1024 * 3 + extra]).unwrap();
        compare(DynamicImage::ImageRgb8(rgb), true);
        let rgba = image::RgbaImage::from_raw(1, 1024, vec![29; 1024 * 4 + extra]).unwrap();
        compare(DynamicImage::ImageRgba8(rgba), true);
    }
}

#[test]
fn thumbnail_identity_does_not_skip_upscaling() {
    let actual = thumbnail_rgb(pattern(1, 1, false, 123));
    assert_eq!(actual.dimensions(), (1024, 1024));
    compare(pattern(1023, 1, true, 456), true);
}

#[test]
fn thumbnail_identity_other_formats_and_empty_images_use_reference() {
    let rgb = pattern(1024, 3, false, 0x1234_5678);
    let formats = [
        DynamicImage::ImageLuma8(rgb.to_luma8()),
        DynamicImage::ImageLumaA8(rgb.to_luma_alpha8()),
        DynamicImage::ImageLuma16(rgb.to_luma16()),
        DynamicImage::ImageLumaA16(rgb.to_luma_alpha16()),
        DynamicImage::ImageRgb16(rgb.to_rgb16()),
        DynamicImage::ImageRgba16(rgb.to_rgba16()),
        DynamicImage::ImageRgb32F(rgb.to_rgb32f()),
        DynamicImage::ImageRgba32F(rgb.to_rgba32f()),
    ];
    for image in formats {
        compare(image, true);
    }
    for (width, height) in [(0, 0), (0, 1), (1, 0), (0, 1024), (1024, 0)] {
        for rgba in [false, true] {
            compare(pattern(width, height, rgba, 1), false);
        }
    }
}

#[test]
#[ignore = "paired timing benchmark, run with unchanged selfdev profile and --nocapture"]
fn thumbnail_identity_paired_benchmark() {
    use std::{
        hint::black_box,
        time::{Duration, Instant},
    };
    // Both implementations live in the same binary/profile. Clone before
    // starting the timer because production passes an already-owned image.
    // Alternate execution order to reduce warmup/order bias. No JPEG timing.
    for (name, width, height, rgba) in [
        ("identity-rgb", 1024, 768, false),
        ("identity-rgba", 768, 1024, true),
        ("fallback-upscale", 1023, 768, false),
        ("fallback-downscale", 1025, 768, true),
    ] {
        let input = pattern(width, height, rgba, 0x1234_5678);
        compare(input.clone(), false);
        let mut baseline = Duration::ZERO;
        let mut candidate = Duration::ZERO;
        for round in 0..42 {
            for optimized in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let owned = black_box(input.clone());
                let start = Instant::now();
                let output = if optimized {
                    thumbnail_rgb(owned)
                } else {
                    reference(owned)
                };
                black_box(&output);
                let elapsed = start.elapsed();
                if round >= 2 {
                    if optimized {
                        candidate += elapsed;
                    } else {
                        baseline += elapsed;
                    }
                }
            }
        }
        eprintln!(
            "{name}: paired n=40 baseline={baseline:?} candidate={candidate:?} ratio={:.4}",
            candidate.as_secs_f64() / baseline.as_secs_f64()
        );
    }
}
