//! Full decoder, resize, repeated JPEG and base64 parity against the frozen pipeline.
use super::{WireImage, image_thumbnail};
use base64::Engine;
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;

fn frozen_rgb(image: DynamicImage) -> image::RgbImage {
    let (width, height) = (image.width(), image.height());
    let identity = width != 0
        && height != 0
        && ((width == 1024 && height <= 1024) || (height == 1024 && width <= 1024));
    let storage = match &image {
        DynamicImage::ImageRgb8(rgb) => rgb.as_raw().len() == width as usize * height as usize * 3,
        DynamicImage::ImageRgba8(_) => true,
        _ => false,
    };
    if identity && storage {
        image.into_rgb8()
    } else {
        image.thumbnail(1024, 1024).into_rgb8()
    }
}

fn frozen_thumbnail(bytes: &[u8]) -> (Option<WireImage>, usize) {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .unwrap();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let Ok(decoded) = reader.decode() else {
        return (None, 0);
    };
    let mut thumbnail = frozen_rgb(decoded);
    let mut passes = 0;
    loop {
        passes += 1;
        let mut bytes = Vec::new();
        if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(&thumbnail)
            .is_err()
        {
            return (None, passes);
        }
        if bytes.len() <= 256 * 1024 {
            return (
                Some(WireImage {
                    media_type: "image/jpeg".into(),
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                }),
                passes,
            );
        }
        thumbnail = image::imageops::thumbnail(
            &thumbnail,
            (thumbnail.width() / 2).max(1),
            (thumbnail.height() / 2).max(1),
        );
    }
}

fn fixture(width: u32, height: u32, rgba: bool) -> DynamicImage {
    let mut state = 0x19a4_283bu32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as u8
    };
    if rgba {
        DynamicImage::ImageRgba8(image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                next(),
                next(),
                next(),
                if (x + y) % 3 == 0 { 0 } else { next() },
            ])
        }))
    } else {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |_, _| {
            image::Rgb([next(), next(), next()])
        }))
    }
}

#[test]
fn full_thumbnail_pipeline_preserves_exact_base64_and_retry_output() {
    let mut exercised_retry = false;
    for (width, height, rgba) in [
        (1024, 1024, false),
        (1024, 1024, true),
        (1301, 1057, true),
        (17, 23, true),
        (1024, 3, false),
    ] {
        let mut encoded = Cursor::new(Vec::new());
        fixture(width, height, rgba)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let (expected, passes) = frozen_thumbnail(encoded.get_ref());
        exercised_retry |= passes > 1;
        let actual = image_thumbnail(Cursor::new(encoded.get_ref()));
        assert_eq!(actual, expected, "{width}x{height}, rgba={rgba}");
    }
    assert!(
        exercised_retry,
        "fixtures must exercise repeated JPEG downsizing"
    );
}

#[test]
fn full_thumbnail_pipeline_preserves_invalid_image_rejection() {
    for bytes in [b"".as_slice(), b"not an image", b"\x89PNG\r\n\x1a\n"] {
        assert_eq!(
            image_thumbnail(Cursor::new(bytes)),
            frozen_thumbnail(bytes).0
        );
    }
}

#[test]
#[ignore = "paired unchanged-selfdev complete image pipeline benchmark"]
fn full_thumbnail_pipeline_abba_benchmark() {
    use std::hint::black_box;
    use std::time::{Duration, Instant};
    for (width, height, rgba) in [
        (1024, 1024, false),
        (1024, 1024, true),
        (1920, 1080, false),
        (1920, 1080, true),
        (17, 23, true),
    ] {
        let mut encoded = Cursor::new(Vec::new());
        fixture(width, height, rgba)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let encoded = encoded.into_inner();
        let (expected, passes) = frozen_thumbnail(&encoded);
        assert_eq!(image_thumbnail(Cursor::new(encoded.as_slice())), expected);
        let measure = |candidate| {
            let start = Instant::now();
            if candidate {
                black_box(image_thumbnail(Cursor::new(black_box(encoded.as_slice()))));
            } else {
                black_box(frozen_thumbnail(black_box(&encoded)));
            }
            start.elapsed()
        };
        measure(false);
        measure(true);
        let mut old_total = Duration::ZERO;
        let mut new_total = Duration::ZERO;
        let mut ratios = Vec::new();
        for round in 0..5 {
            let a1 = measure(false);
            let b1 = measure(true);
            let b2 = measure(true);
            let a2 = measure(false);
            let old = a1 + a2;
            let new = b1 + b2;
            old_total += old;
            new_total += new;
            ratios.push(new.as_secs_f64() / old.as_secs_f64());
            println!(
                "PIPELINE_ABBA {width}x{height} rgba={rgba} passes={passes} round={round} old_ns={} new_ns={}",
                old.as_nanos(),
                new.as_nanos()
            );
        }
        ratios.sort_by(f64::total_cmp);
        println!(
            "PIPELINE_SUMMARY {width}x{height} rgba={rgba} passes={passes} old_ms={:.3} new_ms={:.3} median_ratio={:.6}",
            old_total.as_secs_f64() * 1000.0,
            new_total.as_secs_f64() * 1000.0,
            ratios[2]
        );
    }
}
