//! Finished images: PNG output and contact sheets.

use std::path::Path;

/// An RGBA8 image.
#[derive(Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(w: u32, h: u32, fill: [u8; 4]) -> Image {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            rgba.extend_from_slice(&fill);
        }
        Image { w, h, rgba }
    }

    /// Writes a PNG, creating parent directories.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        if let Some(d) = path.parent()
            && !d.as_os_str().is_empty()
        {
            std::fs::create_dir_all(d).map_err(|e| err(&e))?;
        }
        let file = std::fs::File::create(path).map_err(|e| err(&e))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.w, self.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| err(&e))?;
        wr.write_image_data(&self.rgba).map_err(|e| err(&e))?;
        wr.finish().map_err(|e| err(&e))
    }

    /// Images in a grid of `cols`, in order, so one image can show several views.
    pub fn grid(images: &[Image], cols: usize) -> Image {
        let gap = 6u32;
        let cols = cols.max(1);
        let rows = images.len().div_ceil(cols);
        let col_w: Vec<u32> = (0..cols)
            .map(|c| {
                images
                    .iter()
                    .skip(c)
                    .step_by(cols)
                    .map(|i| i.w)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let row_h: Vec<u32> = (0..rows)
            .map(|r| {
                images
                    .iter()
                    .skip(r * cols)
                    .take(cols)
                    .map(|i| i.h)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let total_w = col_w.iter().sum::<u32>() + gap * (cols as u32).saturating_sub(1);
        let total_h = row_h.iter().sum::<u32>() + gap * (rows as u32).saturating_sub(1);
        let mut out = Image::new(total_w, total_h, [20, 23, 26, 255]);
        let mut y0 = 0;
        for r in 0..rows {
            let mut x0 = 0;
            for c in 0..cols {
                if let Some(img) = images.get(r * cols + c) {
                    for y in 0..img.h {
                        let s = (y * img.w * 4) as usize;
                        let d = (((y0 + y) * total_w + x0) * 4) as usize;
                        let n = (img.w * 4) as usize;
                        out.rgba[d..d + n].copy_from_slice(&img.rgba[s..s + n]);
                    }
                }
                x0 += col_w[c] + gap;
            }
            y0 += row_h[r] + gap;
        }
        out
    }
}
