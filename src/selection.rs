/// Rectangular marquee or magic-wand (connected) region in document space.
pub type RegionOutlineSegment = (i32, i32, i32, i32);

#[derive(Clone)]
pub enum Selection {
    Rect(i32, i32, i32, i32),
    /// `mask.len() == width * height`; non-zero = selected (same size as the document when created).
    Region {
        width: u32,
        height: u32,
        mask: Vec<u8>,
        /// When set (e.g. magic wand), tight bounds of `mask` to skip a full-mask scan.
        tight_bbox: Option<(i32, i32, i32, i32)>,
        /// Cached mask boundary line segments `(x0, y0, x1, y1)` for fast marquee redraws.
        outline_segments: Vec<RegionOutlineSegment>,
    },
}

impl Selection {
    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        let xi = x.floor() as i32;
        let yi = y.floor() as i32;
        match self {
            Selection::Rect(sx, sy, sw, sh) => {
                xi >= *sx && yi >= *sy && xi < sx + sw && yi < sy + sh
            }
            Selection::Region {
                width,
                height,
                mask,
                ..
            } => {
                if xi < 0 || yi < 0 || xi >= *width as i32 || yi >= *height as i32 {
                    return false;
                }
                let idx = (yi as u32 * width + xi as u32) as usize;
                mask.get(idx).copied().unwrap_or(0) != 0
            }
        }
    }
}

/// Build merged axis-aligned boundary segments for a region mask.
pub fn region_mask_outline_segments(mask: &[u8], rw: u32, rh: u32) -> Vec<RegionOutlineSegment> {
    let ww = rw as usize;
    let h = rh as usize;
    if mask.len() != ww * h {
        return Vec::new();
    }
    let mut out = Vec::new();

    for y in 0..h {
        let mut x = 0usize;
        while x < ww {
            let idx = y * ww + x;
            if mask[idx] == 0 || !(y == 0 || mask[idx - ww] == 0) {
                x += 1;
                continue;
            }
            let x0 = x;
            x += 1;
            while x < ww {
                let i2 = y * ww + x;
                if mask[i2] == 0 || !(y == 0 || mask[i2 - ww] == 0) {
                    break;
                }
                x += 1;
            }
            out.push((x0 as i32, y as i32, x as i32, y as i32));
        }
    }

    for y in 0..h {
        let mut x = 0usize;
        while x < ww {
            let idx = y * ww + x;
            if mask[idx] == 0 || !(y + 1 == h || mask[idx + ww] == 0) {
                x += 1;
                continue;
            }
            let x0 = x;
            x += 1;
            while x < ww {
                let i2 = y * ww + x;
                if mask[i2] == 0 || !(y + 1 == h || mask[i2 + ww] == 0) {
                    break;
                }
                x += 1;
            }
            out.push((x0 as i32, (y + 1) as i32, x as i32, (y + 1) as i32));
        }
    }

    for x in 0..ww {
        let mut y = 0usize;
        while y < h {
            let idx = y * ww + x;
            if mask[idx] == 0 || !(x == 0 || mask[idx - 1] == 0) {
                y += 1;
                continue;
            }
            let y0 = y;
            y += 1;
            while y < h {
                let i2 = y * ww + x;
                if mask[i2] == 0 || !(x == 0 || mask[i2 - 1] == 0) {
                    break;
                }
                y += 1;
            }
            out.push((x as i32, y0 as i32, x as i32, y as i32));
        }
    }

    for x in 0..ww {
        let mut y = 0usize;
        while y < h {
            let idx = y * ww + x;
            if mask[idx] == 0 || !(x + 1 == ww || mask[idx + 1] == 0) {
                y += 1;
                continue;
            }
            let y0 = y;
            y += 1;
            while y < h {
                let i2 = y * ww + x;
                if mask[i2] == 0 || !(x + 1 == ww || mask[i2 + 1] == 0) {
                    break;
                }
                y += 1;
            }
            out.push(((x + 1) as i32, y0 as i32, (x + 1) as i32, y as i32));
        }
    }

    out
}
