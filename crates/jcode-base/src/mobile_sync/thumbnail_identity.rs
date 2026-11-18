//! Narrow byte-exact shortcut for thumbnails whose resize is an identity.

/// Preserve `image.thumbnail(1024, 1024).into_rgb8()` including upscaling.
pub(super) fn thumbnail_rgb(image: image::DynamicImage) -> image::RgbImage {
    let (width, height) = (image.width(), image.height());
    // image 0.25.9: resize_dimensions chooses min(1024/w, 1024/h),
    // without capping the ratio at one. Only these nonempty dimensions have
    // ratio exactly one. The u8 thumbnail sampler averages a single pixel,
    // exactly preserving every channel (including RGB under zero alpha).
    // Do not extend this to floats, other formats, or smaller images.
    let identity_dimensions = width != 0
        && height != 0
        && ((width == 1024 && height <= 1024) || (height == 1024 && width <= 1024));
    let supported_storage = match &image {
        // from_raw accepts trailing storage. The reference allocates a tight
        // buffer, whereas consuming RGB would otherwise retain those bytes.
        image::DynamicImage::ImageRgb8(rgb) => {
            rgb.as_raw().len() == width as usize * height as usize * 3
        }
        image::DynamicImage::ImageRgba8(_) => true,
        _ => false,
    };
    if identity_dimensions && supported_storage {
        image.into_rgb8()
    } else {
        image.thumbnail(1024, 1024).into_rgb8()
    }
}

#[cfg(test)]
#[path = "thumbnail_identity_tests.rs"]
mod tests;
