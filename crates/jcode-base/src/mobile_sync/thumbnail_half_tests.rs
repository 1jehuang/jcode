use super::half;
use image::{Rgb, RgbImage};
use std::{hint::black_box, time::Instant};

fn reference(image: &RgbImage) -> RgbImage {
    image::imageops::thumbnail(
        image,
        (image.width() / 2).max(1),
        (image.height() / 2).max(1),
    )
}

fn fixture(width: u32, height: u32) -> RgbImage {
    let mut state = 0x748a_f392u32;
    RgbImage::from_fn(width, height, |_, _| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        Rgb([state as u8, (state >> 8) as u8, (state >> 16) as u8])
    })
}

#[test]
fn half_matches_pixels_for_even_odd_empty_and_trailing_storage() {
    for width in 0..=65 {
        for height in 0..=65 {
            let image = fixture(width, height);
            assert_eq!(half(&image), reference(&image), "{width}x{height}");
        }
    }
    for (width, height) in [(1024, 1024), (1024, 576), (1023, 769), (2, 2048)] {
        let image = fixture(width, height);
        assert_eq!(half(&image), reference(&image));
    }
    let image = RgbImage::from_raw(4, 2, vec![173; 4 * 2 * 3 + 7]).unwrap();
    assert_eq!(half(&image), reference(&image));
}

#[test]
fn half_preserves_every_possible_four_pixel_sum_and_jpeg() {
    let image = RgbImage::from_fn(2042, 2, |x, y| {
        let sum = x / 2;
        let cell = y * 2 + x % 2;
        let value = sum.saturating_sub(cell * 255).min(255) as u8;
        Rgb([value, 255 - value, value])
    });
    let actual = half(&image);
    let expected = reference(&image);
    assert_eq!(actual, expected);
    let jpeg = |image: &RgbImage| {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(image)
            .unwrap();
        bytes
    };
    assert_eq!(jpeg(&actual), jpeg(&expected));
}

#[test]
fn half_preserves_color_space_in_fast_and_fallback_paths() {
    use image::metadata::Cicp;
    for color in [Cicp::SRGB, Cicp::SRGB_LINEAR, Cicp::DISPLAY_P3] {
        for (width, height) in [(64, 48), (63, 47), (1024, 576), (2, 2048), (0, 2)] {
            let mut image = fixture(width, height);
            image.set_color_space(color).unwrap();
            let expected = reference(&image);
            let actual = half(&image);
            assert_eq!(actual.color_space(), expected.color_space());
            assert_eq!(actual.color_space(), color);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
#[ignore = "paired unchanged-selfdev exact half-size component benchmark"]
fn half_abba_benchmark() {
    for (width, height) in [(2, 2), (64, 64), (1024, 1024), (1024, 576), (1023, 769)] {
        let image = fixture(width, height);
        assert_eq!(half(&image), reference(&image));
        let iterations = (1_000_000 / (width as usize * height as usize)).clamp(1, 1000);
        let measure = |candidate| {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(if candidate {
                    half(black_box(&image))
                } else {
                    reference(black_box(&image))
                });
            }
            start.elapsed()
        };
        measure(false);
        measure(true);
        let mut ratios = Vec::new();
        for round in 0..7 {
            let a1 = measure(false);
            let b1 = measure(true);
            let b2 = measure(true);
            let a2 = measure(false);
            let old = a1 + a2;
            let new = b1 + b2;
            ratios.push(new.as_secs_f64() / old.as_secs_f64());
            println!(
                "HALF_ABBA {width}x{height} round={round} old_ns={} new_ns={}",
                old.as_nanos(),
                new.as_nanos()
            );
        }
        ratios.sort_by(f64::total_cmp);
        println!(
            "HALF_SUMMARY {width}x{height} median_ratio={:.6}",
            ratios[3]
        );
    }
}
