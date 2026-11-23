use super::layer::{BlendMode, Layer};
use rayon::prelude::*;

/// Exact `round(x / 255.0)` for all `x in 0..=65025` (`255 * 255`), avoiding integer division.
#[inline(always)]
pub(crate) fn div255(x: u32) -> u32 {
    let t = x + 128;
    (t + (t >> 8)) >> 8
}

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
        blend_layer_premul(
            out,
            &layer.pixels,
            layer.opacity.clamp(0.0, 1.0),
            layer.blend,
        );
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
        blend_layer_premul(
            out,
            &layer.pixels,
            layer.opacity.clamp(0.0, 1.0),
            layer.blend,
        );
    }
}

/// `below` must equal `composite_layers_prefix(..., active)` (or `None` when no visible layers exist
/// below `active`). Copies/zeros it into `out`, then blends `layers[active]` and every layer above.
pub fn composite_layers_from_below_into(
    out: &mut [u8],
    width: u32,
    height: u32,
    layers: &[Layer],
    active: usize,
    below: Option<&[u8]>,
) {
    let len = (width * height * 4) as usize;
    assert_eq!(out.len(), len);
    if let Some(below) = below {
        assert_eq!(below.len(), len);
        out.copy_from_slice(below);
    } else {
        out.fill(0);
    }

    for layer in layers.iter().skip(active) {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
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

fn clipped_region(
    width: u32,
    height: u32,
    rect: (i32, i32, i32, i32),
) -> Option<(usize, usize, usize, usize)> {
    let (x, y, w, h) = rect;
    let x0 = x.max(0).min(width as i32);
    let y0 = y.max(0).min(height as i32);
    let x1 = (x + w).max(0).min(width as i32);
    let y1 = (y + h).max(0).min(height as i32);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some((
            x0 as usize,
            y0 as usize,
            (x1 - x0) as usize,
            (y1 - y0) as usize,
        ))
    }
}

/// Recompute one document rectangle from a cached "below active layer" composite (`None` when no
/// visible layers exist below `active`).
pub fn composite_layers_from_below_region_into(
    out: &mut [u8],
    width: u32,
    height: u32,
    layers: &[Layer],
    active: usize,
    below: Option<&[u8]>,
    rect: (i32, i32, i32, i32),
) {
    let len = (width * height * 4) as usize;
    assert_eq!(out.len(), len);
    if let Some(b) = below {
        assert_eq!(b.len(), len);
    }
    let Some((x, y, rw, rh)) = clipped_region(width, height, rect) else {
        return;
    };
    let stride = width as usize * 4;
    let row_bytes = rw * 4;

    if let Some(below) = below {
        for row in y..y + rh {
            let start = row * stride + x * 4;
            out[start..start + row_bytes].copy_from_slice(&below[start..start + row_bytes]);
        }
    } else {
        for row in y..y + rh {
            let start = row * stride + x * 4;
            out[start..start + row_bytes].fill(0);
        }
    }

    for layer in layers.iter().skip(active) {
        if !layer.visible || layer.opacity <= 0.0 {
            continue;
        }
        debug_assert_eq!(layer.width, width);
        debug_assert_eq!(layer.height, height);
        blend_layer_premul_region(
            out,
            &layer.pixels,
            width,
            (x, y, rw, rh),
            layer.opacity.clamp(0.0, 1.0),
            layer.blend,
        );
    }
}

/// Byte count threshold for Rayon parallelism (pixels are independent).
const PAR_BLEND_BYTES: usize = 256 * 1024 * 4;
/// Row-count / pixel threshold for parallel region blending.
const PAR_REGION_PIXELS: usize = 64 * 1024;

#[inline(always)]
fn blend_premul_normal_full_opacity_px(dst: &mut [u8], src: &[u8]) {
    let sa = src[3] as u32;
    if sa == 0 {
        return;
    }
    if sa == 255 {
        dst[0] = src[0];
        dst[1] = src[1];
        dst[2] = src[2];
        dst[3] = 255;
        return;
    }
    let inv_sa = 255u32 - sa;
    dst[0] = (src[0] as u32 + div255(dst[0] as u32 * inv_sa)).min(255) as u8;
    dst[1] = (src[1] as u32 + div255(dst[1] as u32 * inv_sa)).min(255) as u8;
    dst[2] = (src[2] as u32 + div255(dst[2] as u32 * inv_sa)).min(255) as u8;
    dst[3] = (sa + div255(dst[3] as u32 * inv_sa)).min(255) as u8;
}

#[inline(always)]
fn blend_premul_normal_px(dst: &mut [u8], src: &[u8], op_q: u32) {
    let src_a = src[3] as u32;
    if src_a == 0 {
        return;
    }
    let sa = div255(src_a * op_q);
    if sa == 0 {
        return;
    }
    let sb = div255(src[0] as u32 * op_q);
    let sg = div255(src[1] as u32 * op_q);
    let sr = div255(src[2] as u32 * op_q);
    let inv_sa = 255u32 - sa;
    dst[0] = (sb + div255(dst[0] as u32 * inv_sa)).min(255) as u8;
    dst[1] = (sg + div255(dst[1] as u32 * inv_sa)).min(255) as u8;
    dst[2] = (sr + div255(dst[2] as u32 * inv_sa)).min(255) as u8;
    dst[3] = (sa + div255(dst[3] as u32 * inv_sa)).min(255) as u8;
}

#[inline]
fn blend_normal_slice(dst: &mut [u8], src: &[u8], op_q: u32) {
    if op_q == 255 {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            blend_premul_normal_full_opacity_px(d, s);
        }
    } else {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            blend_premul_normal_px(d, s, op_q);
        }
    }
}

/// Premultiplied source-over with exact integer math.
fn blend_layer_premul_normal_int(dst: &mut [u8], src: &[u8], opacity: f32) {
    let op = opacity.clamp(0.0, 1.0);
    if op <= 0.0 {
        return;
    }
    let op_q = (op * 255.0 + 0.5) as u32;
    if op_q == 0 {
        return;
    }
    if dst.len() >= PAR_BLEND_BYTES {
        const CHUNK: usize = 64 * 1024;
        dst.par_chunks_mut(CHUNK)
            .zip(src.par_chunks(CHUNK))
            .for_each(|(d, s)| blend_normal_slice(d, s, op_q));
    } else {
        blend_normal_slice(dst, src, op_q);
    }
}

#[inline(always)]
fn blend_premul_multiply_add_px(dst: &mut [u8], src: &[u8], op: f32, mode: BlendMode) {
    if src[3] == 0 {
        return;
    }
    let sa = src[3] as f32 * op;
    if sa <= 0.0 {
        return;
    }
    let sr = src[0] as f32 * op;
    let sg = src[1] as f32 * op;
    let sb = src[2] as f32 * op;

    let dr = dst[0] as f32;
    let dg = dst[1] as f32;
    let db = dst[2] as f32;
    let da = dst[3] as f32;

    let (nr, ng, nb, na) = match mode {
        // Premultiplied svg:multiply:
        // C_out = C_s * C_d + C_s * (1 - a_d) + C_d * (1 - a_s)
        // a_out = a_s + a_d * (1 - a_s)
        BlendMode::Multiply => {
            let inv_sa = 1.0 - sa * (1.0 / 255.0);
            let inv_da = 1.0 - da * (1.0 / 255.0);
            let r = sr * dr * (1.0 / 255.0) + sr * inv_da + dr * inv_sa;
            let g = sg * dg * (1.0 / 255.0) + sg * inv_da + dg * inv_sa;
            let b = sb * db * (1.0 / 255.0) + sb * inv_da + db * inv_sa;
            let a = sa + da * inv_sa;
            (r, g, b, a)
        }
        // Premultiplied svg:plus (linear dodge / additive):
        // C_out = min(1, C_s + C_d), a_out = min(1, a_s + a_d)
        BlendMode::Add => {
            let r = (sr + dr).min(255.0);
            let g = (sg + dg).min(255.0);
            let b = (sb + db).min(255.0);
            let a = (sa + da).min(255.0);
            (r, g, b, a)
        }
        BlendMode::Normal => unreachable!("handled above"),
    };

    dst[0] = nr.round().clamp(0.0, 255.0) as u8;
    dst[1] = ng.round().clamp(0.0, 255.0) as u8;
    dst[2] = nb.round().clamp(0.0, 255.0) as u8;
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
    if dst.len() >= PAR_BLEND_BYTES {
        const CHUNK: usize = 64 * 1024;
        dst.par_chunks_mut(CHUNK)
            .zip(src.par_chunks(CHUNK))
            .for_each(|(dc, sc)| {
                for (d, s) in dc.chunks_exact_mut(4).zip(sc.chunks_exact(4)) {
                    blend_premul_multiply_add_px(d, s, op, mode);
                }
            });
    } else {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            blend_premul_multiply_add_px(d, s, op, mode);
        }
    }
}

fn blend_layer_premul_region(
    dst: &mut [u8],
    src: &[u8],
    width: u32,
    rect: (usize, usize, usize, usize),
    opacity: f32,
    mode: BlendMode,
) {
    let op = opacity.clamp(0.0, 1.0);
    if op <= 0.0 {
        return;
    }
    let (x, y, rw, rh) = rect;
    let stride = width as usize * 4;
    let x_off = x * 4;
    let row_bytes = rw * 4;
    let use_par = rw * rh >= PAR_REGION_PIXELS && rh >= 8;

    if mode == BlendMode::Normal {
        let op_q = (op * 255.0 + 0.5) as u32;
        if op_q == 0 {
            return;
        }
        if use_par {
            let dst_rows = &mut dst[y * stride..(y + rh) * stride];
            let src_rows = &src[y * stride..(y + rh) * stride];
            dst_rows
                .par_chunks_exact_mut(stride)
                .zip(src_rows.par_chunks_exact(stride))
                .for_each(|(dr, sr)| {
                    blend_normal_slice(
                        &mut dr[x_off..x_off + row_bytes],
                        &sr[x_off..x_off + row_bytes],
                        op_q,
                    );
                });
        } else {
            for row in y..y + rh {
                let start = row * stride + x_off;
                let end = start + row_bytes;
                blend_normal_slice(&mut dst[start..end], &src[start..end], op_q);
            }
        }
    } else if use_par {
        let dst_rows = &mut dst[y * stride..(y + rh) * stride];
        let src_rows = &src[y * stride..(y + rh) * stride];
        dst_rows
            .par_chunks_exact_mut(stride)
            .zip(src_rows.par_chunks_exact(stride))
            .for_each(|(dr, sr)| {
                for (d, s) in dr[x_off..x_off + row_bytes]
                    .chunks_exact_mut(4)
                    .zip(sr[x_off..x_off + row_bytes].chunks_exact(4))
                {
                    blend_premul_multiply_add_px(d, s, op, mode);
                }
            });
    } else {
        for row in y..y + rh {
            let start = row * stride + x_off;
            let end = start + row_bytes;
            for (d, s) in dst[start..end]
                .chunks_exact_mut(4)
                .zip(src[start..end].chunks_exact(4))
            {
                blend_premul_multiply_add_px(d, s, op, mode);
            }
        }
    }
}

#[inline]
fn swizzle_rgba_to_bgra_slice(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
}

/// Pack premultiplied RGBA (`width`×`height`, row stride `width * 4`) into Cairo
/// `Format::ARgb32` memory (BGRA premultiplied on little-endian). `dst_stride` must come from
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
    if w == 0 || h == 0 {
        return;
    }

    if src_stride * h >= PAR_BLEND_BYTES && h >= 8 {
        dst[..dst_stride * h]
            .par_chunks_exact_mut(dst_stride)
            .zip(src[..src_stride * h].par_chunks_exact(src_stride))
            .for_each(|(dr, sr)| {
                swizzle_rgba_to_bgra_slice(&mut dr[..src_stride], sr);
                if dst_stride > src_stride {
                    dr[src_stride..].fill(0);
                }
            });
    } else {
        for (dr, sr) in dst[..dst_stride * h]
            .chunks_exact_mut(dst_stride)
            .zip(src[..src_stride * h].chunks_exact(src_stride))
        {
            swizzle_rgba_to_bgra_slice(&mut dr[..src_stride], sr);
            if dst_stride > src_stride {
                dr[src_stride..].fill(0);
            }
        }
    }
}

pub fn premul_rgba_to_cairo_argb32_region(
    dst: &mut [u8],
    dst_stride: usize,
    width: u32,
    height: u32,
    src: &[u8],
    rect: (i32, i32, i32, i32),
) {
    let Some((x, y, rw, rh)) = clipped_region(width, height, rect) else {
        return;
    };
    let src_stride = width as usize * 4;
    assert!(src.len() >= src_stride * height as usize);
    assert!(dst.len() >= dst_stride * height as usize);
    let x_off = x * 4;
    let row_bytes = rw * 4;
    for row in y..y + rh {
        let s0 = row * src_stride + x_off;
        let d0 = row * dst_stride + x_off;
        swizzle_rgba_to_bgra_slice(&mut dst[d0..d0 + row_bytes], &src[s0..s0 + row_bytes]);
    }
}

/// Straight RGBA (unpremultiply). `premul` is premultiplied RGBA.
pub fn premul_to_straight_rgba(premul: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; premul.len()];
    premul_to_straight_rgba_into(&mut out, premul);
    out
}

/// Write straight RGBA into `dst` (same length as `premul`). Reuses `dst` allocation.
pub fn premul_to_straight_rgba_into(dst: &mut [u8], premul: &[u8]) {
    assert_eq!(dst.len(), premul.len());
    for (d, s) in dst.chunks_exact_mut(4).zip(premul.chunks_exact(4)) {
        let a = s[3];
        if a == 0 {
            d[0] = 0;
            d[1] = 0;
            d[2] = 0;
            d[3] = 0;
        } else if a == 255 {
            d.copy_from_slice(s);
        } else {
            let a32 = a as u32;
            let half = a32 / 2;
            d[0] = ((s[0] as u32 * 255 + half) / a32).min(255) as u8;
            d[1] = ((s[1] as u32 * 255 + half) / a32).min(255) as u8;
            d[2] = ((s[2] as u32 * 255 + half) / a32).min(255) as u8;
            d[3] = a;
        }
    }
}

/// Convert straight RGBA → premultiplied (for loading PNG into a layer).
pub fn straight_to_premul(straight: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; straight.len()];
    for (d, s) in out.chunks_exact_mut(4).zip(straight.chunks_exact(4)) {
        let a = s[3] as u32;
        if a == 255 {
            d.copy_from_slice(s);
        } else if a == 0 {
            d.fill(0);
        } else {
            d[0] = div255(s[0] as u32 * a) as u8;
            d[1] = div255(s[1] as u32 * a) as u8;
            d[2] = div255(s[2] as u32 * a) as u8;
            d[3] = s[3];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::layer::Layer;

    #[test]
    fn div255_exact_for_all_byte_products() {
        for x in 0u32..=(255 * 255) {
            assert_eq!(div255(x), (x + 127) / 255, "mismatch at x={x}");
        }
    }

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
        composite_layers_from_below_into(&mut inc, w, h, &layers_edited, 1, Some(&below));

        let mut full = vec![0u8; (w * h * 4) as usize];
        composite_layers_into(&mut full, w, h, &layers_edited);

        assert_eq!(inc, full);
    }

    #[test]
    fn incremental_region_from_below_matches_full_composite_region() {
        let w = 8u32;
        let h = 8u32;
        let mut bottom = Layer::new(w, h, "bottom");
        bottom.pixels.fill(0);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                bottom.pixels[i..i + 4].copy_from_slice(&[40 + x as u8, 20 + y as u8, 0, 180]);
            }
        }
        let mut top = Layer::new(w, h, "top");
        top.pixels.fill(0);
        let layers = vec![bottom, top];

        let mut below = vec![0u8; (w * h * 4) as usize];
        composite_layers_prefix_into(&mut below, w, h, &layers, 1);

        let mut layers_edited = layers.clone();
        for y in 2..6 {
            for x in 1..5 {
                let i = ((y * w + x) * 4) as usize;
                layers_edited[1].pixels[i..i + 4].copy_from_slice(&[0, 120, 0, 160]);
            }
        }

        let mut region = below.clone();
        composite_layers_from_below_region_into(
            &mut region,
            w,
            h,
            &layers_edited,
            1,
            Some(&below),
            (1, 2, 4, 4),
        );

        let mut full = vec![0u8; (w * h * 4) as usize];
        composite_layers_into(&mut full, w, h, &layers_edited);
        for y in 2..6 {
            let start = ((y * w + 1) * 4) as usize;
            let end = ((y * w + 5) * 4) as usize;
            assert_eq!(&region[start..end], &full[start..end]);
        }
    }

    #[test]
    fn incremental_region_with_no_below_matches_full_composite() {
        let w = 8u32;
        let h = 8u32;
        let mut base = Layer::new(w, h, "base");
        for y in 1..5 {
            for x in 2..6 {
                let i = ((y * w + x) * 4) as usize;
                base.pixels[i..i + 4].copy_from_slice(&[100, 50, 20, 200]);
            }
        }
        let layers = vec![base];
        let mut region = vec![99u8; (w * h * 4) as usize];
        composite_layers_from_below_region_into(&mut region, w, h, &layers, 0, None, (2, 1, 4, 4));
        let full = composite_layers(w, h, &layers);
        for y in 1..5 {
            let start = ((y * w + 2) * 4) as usize;
            let end = ((y * w + 6) * 4) as usize;
            assert_eq!(&region[start..end], &full[start..end]);
        }
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

    #[test]
    fn multiply_over_transparent_preserves_source_color() {
        let w = 1u32;
        let h = 1u32;
        let base = Layer::new(w, h, "base");
        let mut top = Layer::new(w, h, "top");
        top.pixels.copy_from_slice(&[200, 50, 10, 255]);
        top.blend = BlendMode::Multiply;
        let layers = vec![base, top];
        let out = composite_layers(w, h, &layers);
        assert_eq!(out, vec![200, 50, 10, 255]);
    }

    #[test]
    fn add_preserves_premultiplied_invariant() {
        let w = 1u32;
        let h = 1u32;
        let mut base = Layer::new(w, h, "base");
        base.pixels.copy_from_slice(&[120, 0, 0, 120]);
        let mut top = Layer::new(w, h, "top");
        top.pixels.copy_from_slice(&[120, 0, 0, 120]);
        top.blend = BlendMode::Add;
        let layers = vec![base, top];
        let out = composite_layers(w, h, &layers);
        assert!(out[0] <= out[3], "premul violated: {:?}", out);
        assert_eq!(out, vec![240, 0, 0, 240]);
    }
}
