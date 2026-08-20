//! Gaussian blur for shadows and backdrop effects.
//!
//! tiny-skia has no blur, so this is the approximation the SVG filter spec
//! prescribes and browsers use: three successive box blurs, which converge on a
//! true Gaussian closely enough that the difference is invisible at the radii
//! interface shadows use.

use tiny_skia::Pixmap;

/// Blurs `pixmap` in place.
///
/// `sigma` is the Gaussian standard deviation. CSS gives shadows a *blur
/// radius*, which is twice sigma — a `box-shadow` of `16px` is `sigma = 8`.
pub(crate) fn blur(pixmap: &mut Pixmap, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }

    // The box width the SVG spec derives from sigma.
    let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize;
    if d < 1 {
        return;
    }

    let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data_mut();

    // An odd box is symmetric, so three identical passes centre correctly. An
    // even one cannot be, so the spec offsets two passes and widens the third.
    let passes = if d % 2 == 1 {
        [(d, d / 2), (d, d / 2), (d, d / 2)]
    } else {
        [(d, d / 2), (d, d / 2 - 1), (d + 1, d / 2)]
    };

    let mut scratch = vec![0u8; data.len()];
    for (box_width, offset) in passes {
        box_blur_horizontal(data, &mut scratch, width, height, box_width, offset);
        box_blur_vertical(&scratch, data, width, height, box_width, offset);
    }
}

/// One horizontal box-blur pass, using a sliding window sum per channel.
fn box_blur_horizontal(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    box_width: usize,
    offset: usize,
) {
    if width == 0 || height == 0 {
        return;
    }

    for y in 0..height {
        let row = y * width * 4;
        let mut sums = [0u32; 4];

        // Prime the window, clamping at the edges so borders do not darken.
        for i in 0..box_width {
            let x = i.saturating_sub(offset).min(width - 1);
            for (channel, sum) in sums.iter_mut().enumerate() {
                *sum += u32::from(src[row + x * 4 + channel]);
            }
        }

        for x in 0..width {
            for (channel, sum) in sums.iter().enumerate() {
                dst[row + x * 4 + channel] = (*sum / box_width as u32) as u8;
            }

            let leaving = x.saturating_sub(offset).min(width - 1);
            let entering = (x + box_width - offset).min(width - 1);
            for (channel, sum) in sums.iter_mut().enumerate() {
                *sum += u32::from(src[row + entering * 4 + channel]);
                *sum -= u32::from(src[row + leaving * 4 + channel]);
            }
        }
    }
}

/// The same pass down each column.
fn box_blur_vertical(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    box_width: usize,
    offset: usize,
) {
    if width == 0 || height == 0 {
        return;
    }

    for x in 0..width {
        let column = x * 4;
        let mut sums = [0u32; 4];

        for i in 0..box_width {
            let y = i.saturating_sub(offset).min(height - 1);
            for (channel, sum) in sums.iter_mut().enumerate() {
                *sum += u32::from(src[y * width * 4 + column + channel]);
            }
        }

        for y in 0..height {
            for (channel, sum) in sums.iter().enumerate() {
                dst[y * width * 4 + column + channel] = (*sum / box_width as u32) as u8;
            }

            let leaving = y.saturating_sub(offset).min(height - 1);
            let entering = (y + box_width - offset).min(height - 1);
            for (channel, sum) in sums.iter_mut().enumerate() {
                *sum += u32::from(src[entering * width * 4 + column + channel]);
                *sum -= u32::from(src[leaving * width * 4 + column + channel]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, FillRule, Paint, PathBuilder, Transform};

    /// A pixmap with an opaque white square in the middle of a black field.
    fn square() -> Pixmap {
        let mut pixmap = Pixmap::new(64, 64).unwrap();
        pixmap.fill(Color::BLACK);

        let mut builder = PathBuilder::new();
        builder.push_rect(tiny_skia::Rect::from_ltrb(24.0, 24.0, 40.0, 40.0).unwrap());
        let path = builder.finish().unwrap();

        let mut paint = Paint::default();
        paint.set_color(Color::WHITE);
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        pixmap
    }

    fn red_at(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
        pixmap.pixel(x, y).unwrap().red()
    }

    #[test]
    fn a_zero_sigma_leaves_the_image_alone() {
        let mut blurred = square();
        blur(&mut blurred, 0.0);
        assert_eq!(blurred.data(), square().data());
    }

    #[test]
    fn blurring_spreads_the_edge_outwards() {
        let mut blurred = square();
        // Just outside the square, so it is black before blurring.
        assert_eq!(red_at(&blurred, 22, 32), 0);

        blur(&mut blurred, 4.0);

        assert!(
            red_at(&blurred, 22, 32) > 0,
            "the edge should bleed past its original bounds"
        );
        assert!(
            red_at(&blurred, 32, 32) < 255,
            "the centre should lose intensity to its surroundings"
        );
    }

    #[test]
    fn a_flat_field_survives_unchanged() {
        // Every window averages the same value, so nothing may shift — this is
        // what catches an off-by-one in the sliding window.
        let mut flat = Pixmap::new(32, 32).unwrap();
        flat.fill(Color::from_rgba8(40, 80, 120, 255));

        blur(&mut flat, 3.0);

        for pixel in flat.pixels() {
            assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (40, 80, 120));
        }
    }
}
