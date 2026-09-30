use radiust_core::model::Preview;
use std::fmt::Write as _;

/// Render an RGBA preview using ANSI true color and the upper-half block.
/// The preview is downsampled with nearest-neighbor selection to keep terminal
/// output bounded while retaining the original image's aspect ratio.
pub fn render_ansi(
    preview: &Preview,
    max_columns: usize,
    max_rows: usize,
) -> Result<String, String> {
    let expected = (preview.width as usize)
        .checked_mul(preview.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "preview dimensions overflow".to_owned())?;
    if preview.width == 0 || preview.height == 0 || preview.rgba.len() != expected {
        return Err("preview RGBA data does not match its dimensions".into());
    }
    if max_columns == 0 || max_rows == 0 {
        return Err("terminal preview dimensions must be positive".into());
    }

    let available_height = max_rows.saturating_mul(2);
    let scale = (max_columns as f64 / preview.width as f64)
        .min(available_height as f64 / preview.height as f64)
        .min(1.0);
    let width = ((preview.width as f64 * scale).round() as usize).max(1);
    let height = ((preview.height as f64 * scale).round() as usize).max(1);
    let mut output =
        String::with_capacity(width.saturating_mul(height.div_ceil(2)).saturating_mul(44));

    for row in (0..height).step_by(2) {
        for column in 0..width {
            let top = sample(preview, column, row, width, height);
            let bottom = sample(preview, column, (row + 1).min(height - 1), width, height);
            write!(
                output,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                top[0], top[1], top[2], bottom[0], bottom[1], bottom[2]
            )
            .map_err(|_| "terminal preview formatting failed".to_owned())?;
        }
        output.push_str("\x1b[0m\n");
    }
    Ok(output)
}

fn sample(preview: &Preview, x: usize, y: usize, width: usize, height: usize) -> [u8; 3] {
    let source_x = ((x * preview.width as usize) / width).min(preview.width as usize - 1);
    let source_y = ((y * preview.height as usize) / height).min(preview.height as usize - 1);
    let offset = (source_y * preview.width as usize + source_x) * 4;
    let rgba: [u8; 4] =
        preview.rgba[offset..offset + 4].try_into().expect("RGBA pixel has four channels");
    let alpha = u16::from(rgba[3]);
    [
        (u16::from(rgba[0]) * alpha / 255) as u8,
        (u16::from(rgba[1]) * alpha / 255) as u8,
        (u16::from(rgba[2]) * alpha / 255) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiust_core::model::PreviewMode;

    fn preview(width: u32, height: u32, rgba: Vec<u8>) -> Preview {
        Preview { width, height, rgba, frame: None, mode: PreviewMode::Raw, rule_version: None }
    }

    #[test]
    fn renders_two_rgba_rows_per_terminal_cell_and_resets_color_each_line() {
        let image = preview(1, 2, vec![255, 0, 0, 255, 0, 0, 255, 255]);
        let rendered = render_ansi(&image, 10, 10).unwrap();
        assert!(rendered.contains("38;2;255;0;0m"));
        assert!(rendered.contains("48;2;0;0;255m▀"));
        assert!(rendered.ends_with("\x1b[0m\n"));
    }

    #[test]
    fn output_size_is_bounded_by_the_requested_terminal_dimensions() {
        let image = preview(200, 100, vec![7; 200 * 100 * 4]);
        let rendered = render_ansi(&image, 20, 5).unwrap();
        assert_eq!(rendered.matches('▀').count(), 100);
        assert_eq!(rendered.lines().count(), 5);
    }

    #[test]
    fn malformed_rgba_and_zero_sized_terminals_are_rejected() {
        assert!(render_ansi(&preview(1, 1, vec![0, 0, 0]), 10, 10).is_err());
        assert!(render_ansi(&preview(1, 1, vec![0, 0, 0, 0]), 0, 10).is_err());
    }

    #[test]
    fn transparent_pixels_are_composited_without_revealing_hidden_rgb() {
        let image = preview(1, 1, vec![255, 20, 10, 0]);
        let rendered = render_ansi(&image, 10, 10).unwrap();
        assert!(rendered.contains("38;2;0;0;0m"));
        assert!(rendered.contains("48;2;0;0;0m"));
    }
}
