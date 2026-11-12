use std::io::Read;
use std::time::Duration;

use image::{imageops::FilterType, DynamicImage, RgbImage};
use ratatui_image::picker::ProtocolType;

use crate::model::is_valid_id;

const COVER_SIZE: u32 = 1000;

/// A pixel counts as "black bar" if every channel is at or under this.
const BAR_MAX: u8 = 24;
/// Extra pixels cut next to a bar, since JPEG smears the edge a bit.
const BAR_INSET: u32 = 2;

fn dark_row(image: &RgbImage, y: u32, x0: u32, x1: u32) -> bool {
    (x0..x1).all(|x| image.get_pixel(x, y).0.iter().all(|&c| c <= BAR_MAX))
}

fn dark_col(image: &RgbImage, x: u32, y0: u32, y1: u32) -> bool {
    (y0..y1).all(|y| image.get_pixel(x, y).0.iter().all(|&c| c <= BAR_MAX))
}

/// YouTube thumbnails like to put the real art inside black bars. This
/// finds them and crops them off.
fn trim_black_bars(image: DynamicImage) -> DynamicImage {
    let rgb = image.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w < 16 || h < 16 {
        return image;
    }
    let top = (0..h).take_while(|&y| dark_row(&rgb, y, 0, w)).count() as u32;
    if top == h {
        return image;
    }
    let bottom = (0..h)
        .rev()
        .take_while(|&y| dark_row(&rgb, y, 0, w))
        .count() as u32;
    let (y0, y1) = (top, h - bottom);
    let left = (0..w).take_while(|&x| dark_col(&rgb, x, y0, y1)).count() as u32;
    let right = (0..w)
        .rev()
        .take_while(|&x| dark_col(&rgb, x, y0, y1))
        .count() as u32;
    let inset = |bar: u32| if bar > 0 { bar + BAR_INSET } else { 0 };
    let (top, bottom, left, right) = (inset(top), inset(bottom), inset(left), inset(right));
    if top + bottom >= h || left + right >= w {
        return image;
    }
    let (new_w, new_h) = (w - left - right, h - top - bottom);
    // if that would eat half the image it's just a dark cover, leave it
    if new_w * 2 < w || new_h * 2 < h {
        return image;
    }
    image.crop_imm(left, top, new_w, new_h)
}

fn square_cover(image: DynamicImage) -> RgbImage {
    trim_black_bars(image)
        .resize_to_fill(COVER_SIZE, COVER_SIZE, FilterType::Lanczos3)
        .to_rgb8()
}

/// Tiny copy of the cover, used when the terminal can't show real images.
pub fn thumbnail(image: &RgbImage) -> RgbImage {
    image::imageops::resize(image, 128, 128, FilterType::Triangle)
}

pub fn supports_protocol(protocol: ProtocolType) -> bool {
    matches!(protocol, ProtocolType::Kitty | ProtocolType::Iterm2)
}

/// Use the same track artwork as Discord, with a small decoded image kept in memory.
pub fn fetch(id: &str) -> Option<RgbImage> {
    if !is_valid_id(id) {
        return None;
    }
    let response = ureq::get(&format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
        .timeout(Duration::from_secs(10))
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    reader.limits(limits);
    Some(square_cover(reader.decode().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_a_real_image_protocol() {
        assert!(supports_protocol(ProtocolType::Kitty));
        assert!(supports_protocol(ProtocolType::Iterm2));
        assert!(!supports_protocol(ProtocolType::Halfblocks));
        assert!(!supports_protocol(ProtocolType::Sixel));
    }

    #[test]
    fn creates_both_graphics_protocols() {
        for protocol in [ProtocolType::Kitty, ProtocolType::Iterm2] {
            let mut picker = ratatui_image::picker::Picker::from_fontsize((10, 20));
            picker.set_protocol_type(protocol);
            let image = image::RgbImage::from_pixel(16, 16, image::Rgb([255, 0, 0]));
            let state = picker.new_resize_protocol(image.into());
            match (protocol, state.protocol_type()) {
                (ProtocolType::Kitty, ratatui_image::protocol::StatefulProtocolType::Kitty(_))
                | (
                    ProtocolType::Iterm2,
                    ratatui_image::protocol::StatefulProtocolType::ITerm2(_),
                ) => {}
                _ => panic!("unexpected graphics protocol state"),
            }
        }
    }

    #[test]
    fn removes_black_bars_above_and_below_the_art() {
        // 4:3 thumbnail: black bars top and bottom, art with brown sides
        let image = RgbImage::from_fn(480, 360, |x, y| {
            if !(45..315).contains(&y) {
                image::Rgb([0, 0, 0])
            } else if (90..390).contains(&x) {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([90, 30, 10])
            }
        });
        let cover = square_cover(image.into());
        assert_eq!(cover.dimensions(), (1000, 1000));
        for (x, y) in [(0, 0), (999, 0), (0, 999), (999, 999), (500, 0), (500, 999)] {
            assert_eq!(cover.get_pixel(x, y).0, [255, 255, 255], "pixel {x},{y}");
        }
    }

    #[test]
    fn keeps_dark_art_without_bars() {
        let image = RgbImage::from_pixel(480, 360, image::Rgb([5, 5, 5]));
        assert_eq!(square_cover(image.into()).dimensions(), (1000, 1000));
    }

    #[test]
    fn crops_letterboxed_art_to_a_square() {
        let image = RgbImage::from_fn(1600, 900, |x, _| {
            if (350..1250).contains(&x) {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        let cover = square_cover(image.into());
        assert_eq!(cover.dimensions(), (1000, 1000));
        assert_eq!(cover.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(cover.get_pixel(999, 999).0, [255, 255, 255]);
    }
}
