/// Rectangular marquee or magic-wand (connected) region in document space.
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
