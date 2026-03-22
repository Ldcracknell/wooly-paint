use super::layer::{BlendMode, Layer};

/// Composite visible layers bottom → top into premultiplied RGBA8 (same size as document).
pub fn composite_layers(width: u32, height: u32, layers: &[Layer]) -> Vec<u8> {
    let len = (width * height * 4) as usize;
    let mut out = vec![0u8; len];
    composite_layers_into(&mut out, width, height, layers);
    out
}

/// Composite into `out` (length must be `width * height * 4`). Reuses allocation.
pub fn composite_layers_into(out: &mut [u8], width: u32, height: u32, layers: &[Layer]) {
    let len = (width * height * 4) as usize;
    assert_eq!(out.len(), len);
    out.fill(0);

    for layer in layers {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        debug_assert_eq!(layer.width, width);
        debug_assert_eq!(layer.height, height);
        blend_layer_premul(out, &layer.pixels, layer.opacity.clamp(0.0, 1.0), layer.blend);
    }
}

pub fn blend_layer_premul(dst: &mut [u8], src: &[u8], opacity: f32, mode: BlendMode) {
    let op = opacity as f32;
    for i in (0..dst.len()).step_by(4) {
        let cb = dst[i] as f32;
        let cg = dst[i + 1] as f32;
        let cr = dst[i + 2] as f32;
        let ca = dst[i + 3] as f32;

        let sb = src[i] as f32 * op;
        let sg = src[i + 1] as f32 * op;
        let sr = src[i + 2] as f32 * op;
        let sa = src[i + 3] as f32 * op;

        if sa <= 0.0 {
            continue;
        }

        let (nb, ng, nr, na) = match mode {
            BlendMode::Normal => {
                let inv = 1.0 - sa / 255.0;
                let nb = sb + cb * inv;
                let ng = sg + cg * inv;
                let nr = sr + cr * inv;
                let na = sa + ca * inv;
                (nb, ng, nr, na)
            }
            BlendMode::Multiply => {
                let b = sb * cb / 255.0 + cb * (1.0 - sa / 255.0);
                let g = sg * cg / 255.0 + cg * (1.0 - sa / 255.0);
                let r = sr * cr / 255.0 + cr * (1.0 - sa / 255.0);
                let a = sa + ca * (1.0 - sa / 255.0);
                (b, g, r, a)
            }
            BlendMode::Add => {
                let b = (sb + cb).min(255.0);
                let g = (sg + cg).min(255.0);
                let r = (sr + cr).min(255.0);
                let a = (sa + ca - sa * ca / 255.0).min(255.0);
                (b, g, r, a)
            }
        };

        dst[i] = nb.round().clamp(0.0, 255.0) as u8;
        dst[i + 1] = ng.round().clamp(0.0, 255.0) as u8;
        dst[i + 2] = nr.round().clamp(0.0, 255.0) as u8;
        dst[i + 3] = na.round().clamp(0.0, 255.0) as u8;
    }
}

/// Straight RGBA for GdkPixbuf (unpremultiply). `premul` is premultiplied RGBA.
pub fn premul_to_straight_rgba(premul: &[u8]) -> Vec<u8> {
    let mut out = premul.to_vec();
    premul_to_straight_rgba_into(&mut out, premul);
    out
}

/// Write straight RGBA into `dst` (same length as `premul`). Reuses `dst` allocation.
pub fn premul_to_straight_rgba_into(dst: &mut [u8], premul: &[u8]) {
    assert_eq!(dst.len(), premul.len());
    for i in (0..dst.len()).step_by(4) {
        let a = premul[i + 3];
        if a == 0 {
            dst[i] = 0;
            dst[i + 1] = 0;
            dst[i + 2] = 0;
            dst[i + 3] = 0;
            continue;
        }
        dst[i] = ((premul[i] as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        dst[i + 1] = ((premul[i + 1] as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        dst[i + 2] = ((premul[i + 2] as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        dst[i + 3] = a;
    }
}

/// Convert straight RGBA → premultiplied (for loading PNG into a layer).
pub fn straight_to_premul(straight: &[u8]) -> Vec<u8> {
    let mut out = straight.to_vec();
    for i in (0..out.len()).step_by(4) {
        let a = out[i + 3] as u32;
        out[i] = (out[i] as u32 * a / 255) as u8;
        out[i + 1] = (out[i + 1] as u32 * a / 255) as u8;
        out[i + 2] = (out[i + 2] as u32 * a / 255) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::layer::Layer;

    #[test]
    fn normal_opacity_half_over_white() {
        let w = 1u32;
        let h = 1u32;
        let mut base = Layer::new(w, h, "base");
        base.pixels.copy_from_slice(&[255, 255, 255, 255]);
        let mut top = Layer::new(w, h, "top");
        top.pixels.copy_from_slice(&[128, 0, 0, 128]);
        top.opacity = 1.0;
        let layers = vec![base, top];
        let out = composite_layers(w, h, &layers);
        // Half-opaque red over opaque white → opaque light red / pink.
        assert_eq!(out[0], 255);
        assert!((out[1] as i32 - 127).abs() <= 1);
        assert!((out[2] as i32 - 127).abs() <= 1);
        assert_eq!(out[3], 255);
    }

    #[test]
    fn multiply_red_on_white() {
        let w = 1u32;
        let h = 1u32;
        let mut base = Layer::new(w, h, "base");
        base.pixels.copy_from_slice(&[255, 255, 255, 255]);
        let mut top = Layer::new(w, h, "top");
        top.pixels.copy_from_slice(&[255, 0, 0, 255]);
        top.blend = BlendMode::Multiply;
        let layers = vec![base, top];
        let out = composite_layers(w, h, &layers);
        assert_eq!(out[0..3].to_vec(), vec![255, 0, 0]);
    }
}
