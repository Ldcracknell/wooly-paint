use super::layer::{BlendMode, Layer};
use rayon::prelude::*;

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

/// Composite only layers with indices `0..prefix_len` (i.e. strictly below `active_layer` when
/// `prefix_len == active_layer`).
pub fn composite_layers_prefix_into(
    out: &mut [u8],
    width: u32,
    height: u32,
    layers: &[Layer],
    prefix_len: usize,
) {
    let len = (width * height * 4) as usize;
    assert_eq!(out.len(), len);
    out.fill(0);
    let end = prefix_len.min(layers.len());
    for layer in layers.iter().take(end) {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        debug_assert_eq!(layer.width, width);
        debug_assert_eq!(layer.height, height);
        blend_layer_premul(out, &layer.pixels, layer.opacity.clamp(0.0, 1.0), layer.blend);
    }
}

/// `below` must equal `composite_layers_prefix(..., active)` from stroke start. Copies it into `out`,
/// then blends `layers[active]` and every layer above — same result as a full composite when only
/// `layers[active]` changed since `below` was captured.
pub fn composite_layers_from_below_into(
    out: &mut [u8],
    width: u32,
    height: u32,
    layers: &[Layer],
    active: usize,
    below: &[u8],
) {
    let len = (width * height * 4) as usize;
    assert_eq!(out.len(), len);
    assert_eq!(below.len(), len);
    out.copy_from_slice(below);

    if let Some(layer) = layers.get(active) {
        if layer.visible && layer.opacity > 0.0 {
            debug_assert_eq!(layer.width, width);
            debug_assert_eq!(layer.height, height);
            blend_layer_premul(
                out,
                &layer.pixels,
                layer.opacity.clamp(0.0, 1.0),
                layer.blend,
            );
        }
    }
    for layer in layers.iter().skip(active.saturating_add(1)) {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        debug_assert_eq!(layer.width, width);
        debug_assert_eq!(layer.height, height);
        blend_layer_premul(out, &layer.pixels, layer.opacity.clamp(0.0, 1.0), layer.blend);
    }
}

fn scale_premul_channel(c: u8, op_q: u32) -> u32 {
    (c as u32 * op_q + 127) / 255
}

/// Pixels at or above this use parallel Normal blend (each pixel is independent).
const NORMAL_BLEND_PAR_BYTES: usize = 256 * 1024 * 4;

#[inline]
fn blend_premul_normal_px(dst: &mut [u8], src: &[u8], op_q: u32) {
    let sb = scale_premul_channel(src[0], op_q).min(255);
    let sg = scale_premul_channel(src[1], op_q).min(255);
    let sr = scale_premul_channel(src[2], op_q).min(255);
    let sa = scale_premul_channel(src[3], op_q).min(255);
    if sa == 0 {
        return;
    }
    let inv_sa = 255u32 - sa;
    let cb = dst[0] as u32;
    let cg = dst[1] as u32;
    let cr = dst[2] as u32;
    let ca = dst[3] as u32;
    dst[0] = (sb + (cb * inv_sa + 127) / 255).min(255) as u8;
    dst[1] = (sg + (cg * inv_sa + 127) / 255).min(255) as u8;
    dst[2] = (sr + (cr * inv_sa + 127) / 255).min(255) as u8;
    dst[3] = (sa + (ca * inv_sa + 127) / 255).min(255) as u8;
}

/// Premultiplied source-over with integer math (matches float Normal within rounding).
fn blend_layer_premul_normal_int(dst: &mut [u8], src: &[u8], opacity: f32) {
    let op = opacity.clamp(0.0, 1.0);
    if op <= 0.0 {
        return;
    }
    let op_q = (op * 255.0 + 0.5) as u32;
    if op_q == 0 {
        return;
    }
    if dst.len() >= NORMAL_BLEND_PAR_BYTES {
        dst.par_chunks_mut(4)
            .zip(src.par_chunks(4))
            .for_each(|(d, s)| blend_premul_normal_px(d, s, op_q));
    } else {
        for (d, s) in dst.chunks_mut(4).zip(src.chunks(4)) {
            blend_premul_normal_px(d, s, op_q);
        }
    }
}

#[inline]
fn blend_premul_multiply_add_px(dst: &mut [u8], src: &[u8], op: f32, mode: BlendMode) {
    let cb = dst[0] as f32;
    let cg = dst[1] as f32;
    let cr = dst[2] as f32;
    let ca = dst[3] as f32;

    let sb = src[0] as f32 * op;
    let sg = src[1] as f32 * op;
    let sr = src[2] as f32 * op;
    let sa = src[3] as f32 * op;

    if sa <= 0.0 {
        return;
    }

    let (nb, ng, nr, na) = match mode {
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
        BlendMode::Normal => unreachable!("handled above"),
    };

    dst[0] = nb.round().clamp(0.0, 255.0) as u8;
    dst[1] = ng.round().clamp(0.0, 255.0) as u8;
    dst[2] = nr.round().clamp(0.0, 255.0) as u8;
    dst[3] = na.round().clamp(0.0, 255.0) as u8;
}

pub fn blend_layer_premul(dst: &mut [u8], src: &[u8], opacity: f32, mode: BlendMode) {
    if mode == BlendMode::Normal {
        blend_layer_premul_normal_int(dst, src, opacity);
        return;
    }
    let op = opacity.clamp(0.0, 1.0);
    if op <= 0.0 {
        return;
    }
    if dst.len() >= NORMAL_BLEND_PAR_BYTES {
        dst.par_chunks_mut(4)
            .zip(src.par_chunks(4))
            .for_each(|(d, s)| blend_premul_multiply_add_px(d, s, op, mode));
    } else {
        for (d, s) in dst.chunks_mut(4).zip(src.chunks(4)) {
            blend_premul_multiply_add_px(d, s, op, mode);
        }
    }
}

/// Pack premultiplied RGBA (`width`×`height`, row stride `width * 4`) into Cairo
/// `Format::ARgb32` memory (BGRA premultiplied). `dst_stride` must come from
/// `Format::ARgb32.stride_for_width(width)`.
pub fn premul_rgba_to_cairo_argb32(
    dst: &mut [u8],
    dst_stride: usize,
    width: u32,
    height: u32,
    src: &[u8],
) {
    let w = width as usize;
    let h = height as usize;
    let src_stride = w * 4;
    assert!(src.len() >= src_stride * h);
    assert!(dst.len() >= dst_stride * h);
    for row in 0..h {
        let s0 = row * src_stride;
        let d0 = row * dst_stride;
        for x in 0..w {
            let si = s0 + x * 4;
            let di = d0 + x * 4;
            dst[di] = src[si + 2];
            dst[di + 1] = src[si + 1];
            dst[di + 2] = src[si];
            dst[di + 3] = src[si + 3];
        }
        if dst_stride > src_stride {
            dst[d0 + src_stride..d0 + dst_stride].fill(0);
        }
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
    fn incremental_from_below_matches_full_composite() {
        let w = 8u32;
        let h = 8u32;
        let mut bottom = Layer::new(w, h, "bottom");
        bottom.pixels[0..4].copy_from_slice(&[80, 0, 0, 128]);
        let mut top = Layer::new(w, h, "top");
        top.pixels.fill(0);
        let layers = vec![bottom, top];

        let mut below = vec![0u8; (w * h * 4) as usize];
        composite_layers_prefix_into(&mut below, w, h, &layers, 1);

        let mut layers_edited = layers.clone();
        layers_edited[1].pixels[16..20].copy_from_slice(&[0, 100, 0, 200]);

        let mut inc = vec![0u8; (w * h * 4) as usize];
        composite_layers_from_below_into(&mut inc, w, h, &layers_edited, 1, &below);

        let mut full = vec![0u8; (w * h * 4) as usize];
        composite_layers_into(&mut full, w, h, &layers_edited);

        assert_eq!(inc, full);
    }

    #[test]
    fn premul_rgba_to_cairo_swizzle_and_stride() {
        let w = 3u32;
        let h = 2u32;
        let src: Vec<u8> = vec![
            10, 20, 30, 40, 1, 2, 3, 255, 0, 0, 0, 0, 5, 5, 5, 128, 100, 0, 0, 200, 255, 255, 255,
            255,
        ];
        let stride = 16usize;
        let mut dst = vec![0u8; stride * h as usize];
        premul_rgba_to_cairo_argb32(&mut dst, stride, w, h, &src);
        assert_eq!(&dst[0..4], &[30, 20, 10, 40]);
        assert_eq!(&dst[12..16], &[0u8; 4]);
        assert_eq!(&dst[stride..stride + 4], &[5, 5, 5, 128]);
        assert_eq!(&dst[stride + 4..stride + 8], &[0, 0, 100, 200]);
        assert_eq!(&dst[stride + 8..stride + 12], &[255, 255, 255, 255]);
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
