//! Exact 2-by-2 RGB box averages for the thumbnail size-limit retry.
use image::{GenericImageView, RgbImage};

pub(super) fn half(image: &RgbImage) -> RgbImage {
    let (width, height) = image.dimensions();
    if width == 0
        || height == 0
        || width > 1024
        || height > 1024
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
    {
        return image::imageops::thumbnail(image, (width / 2).max(1), (height / 2).max(1));
    }
    // Inputs outside mobile thumbnail bounds stay on the general path, also
    // bounding all coordinate arithmetic well within exact f32 integer range.
    // The existing imageops thumbnail uses ratio exactly 2 on both axes,
    // averages each independent channel over four pixels, and rounds with +2.
    // No fractional sample or floating-point dimension calculation is involved.
    let stride = width as usize * 3;
    let source = image.as_raw();
    if source.len() != stride * height as usize {
        return image::imageops::thumbnail(image, width / 2, height / 2);
    }
    // Use the same factory as imageops so color-space metadata is retained.
    let mut output = image.buffer_with_dimensions(width / 2, height / 2);
    let target = output.as_mut();
    let mut y = 0usize;
    let mut destination = 0usize;
    while y < height as usize {
        let mut x = 0usize;
        let first = y * stride;
        let second = first + stride;
        while x < stride {
            let mut channel = 0usize;
            while channel < 3 {
                let sum = u16::from(source[first + x + channel])
                    + u16::from(source[first + x + channel + 3])
                    + u16::from(source[second + x + channel])
                    + u16::from(source[second + x + channel + 3]);
                target[destination + channel] = ((sum + 2) / 4) as u8;
                channel += 1;
            }
            x += 6;
            destination += 3;
        }
        y += 2;
    }
    output
}

#[cfg(test)]
#[path = "thumbnail_half_tests.rs"]
mod tests;
