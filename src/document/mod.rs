mod composite;
mod history;
pub mod layer;

pub use composite::{composite_layers, premul_to_straight_rgba, straight_to_premul};
pub use history::History;
pub use layer::{BlendMode, Layer};

use anyhow::{Context, Result};
use image::{ImageBuffer, RgbaImage};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

pub struct Document {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub active_layer: usize,
    pub path: Option<PathBuf>,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        let mut bg = Layer::new(width, height, "Background");
        bg.pixels.fill(255);
        Self {
            width,
            height,
            layers: vec![bg],
            active_layer: 0,
            path: None,
        }
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut Layer> {
        self.layers.get_mut(self.active_layer)
    }

    pub fn active_layer_ref(&self) -> Option<&Layer> {
        self.layers.get(self.active_layer)
    }

    pub fn composite(&self) -> Vec<u8> {
        composite_layers(self.width, self.height, &self.layers)
    }

    pub fn load_png(path: &Path) -> Result<Self> {
        let img = image::open(path)
            .with_context(|| format!("open {}", path.display()))?
            .to_rgba8();
        let (width, height) = img.dimensions();
        let premul = straight_to_premul(img.as_raw());
        let mut layer = Layer::new(width, height, "Background");
        layer.pixels = premul;
        Ok(Self {
            width,
            height,
            layers: vec![layer],
            active_layer: 0,
            path: Some(path.to_path_buf()),
        })
    }

    pub fn save_png(&self, path: &Path) -> Result<()> {
        let comp = self.composite();
        let straight = premul_to_straight_rgba(&comp);
        let buf: RgbaImage = ImageBuffer::from_raw(self.width, self.height, straight)
            .context("buffer size mismatch")?;
        buf.save(path)
            .with_context(|| format!("save {}", path.display()))?;
        Ok(())
    }

    pub fn save_ora(&self, path: &Path) -> Result<()> {
        use zip::write::{SimpleFileOptions, ZipWriter};
        use zip::CompressionMethod;

        let file = std::fs::File::create(path)
            .with_context(|| format!("create {}", path.display()))?;
        let mut zip = ZipWriter::new(file);
        let stored = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored)?;
        zip.write_all(b"image/openraster")?;

        for (i, layer) in self.layers.iter().enumerate() {
            let straight = premul_to_straight_rgba(&layer.pixels);
            let img: RgbaImage =
                ImageBuffer::from_raw(layer.width, layer.height, straight)
                    .context("layer buffer mismatch")?;
            let mut buf = Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png)?;
            zip.start_file(format!("data/layer{i}.png"), deflated)?;
            zip.write_all(&buf.into_inner())?;
        }

        let mut xml = format!(
            "<?xml version='1.0' encoding='UTF-8'?>\n\
             <image version=\"0.0.3\" w=\"{}\" h=\"{}\">\n  <stack>\n",
            self.width, self.height
        );
        for i in (0..self.layers.len()).rev() {
            let l = &self.layers[i];
            let vis = if l.visible { "visible" } else { "hidden" };
            xml.push_str(&format!(
                "    <layer name=\"{}\" src=\"data/layer{}.png\" \
                 opacity=\"{}\" visibility=\"{}\" composite-op=\"{}\"/>\n",
                xml_escape(&l.name),
                i,
                l.opacity,
                vis,
                l.blend.ora_composite_op(),
            ));
        }
        xml.push_str("  </stack>\n</image>\n");
        zip.start_file("stack.xml", deflated)?;
        zip.write_all(xml.as_bytes())?;

        let comp = self.composite();
        let straight = premul_to_straight_rgba(&comp);
        let merged: RgbaImage =
            ImageBuffer::from_raw(self.width, self.height, straight)
                .context("merged buffer mismatch")?;
        let mut buf = Cursor::new(Vec::new());
        merged.write_to(&mut buf, image::ImageFormat::Png)?;
        zip.start_file("mergedimage.png", deflated)?;
        zip.write_all(&buf.into_inner())?;

        let thumb = make_thumbnail(&merged, 256);
        let mut buf = Cursor::new(Vec::new());
        thumb.write_to(&mut buf, image::ImageFormat::Png)?;
        zip.start_file("Thumbnails/thumbnail.png", deflated)?;
        zip.write_all(&buf.into_inner())?;

        zip.finish()?;
        Ok(())
    }

    pub fn load_ora(path: &Path) -> Result<Self> {
        use zip::ZipArchive;

        let file = std::fs::File::open(path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut zip = ZipArchive::new(file).context("invalid zip/ora file")?;

        let mut stack_xml = String::new();
        zip.by_name("stack.xml")
            .context("missing stack.xml")?
            .read_to_string(&mut stack_xml)?;

        let (width, height) = parse_image_dims(&stack_xml)?;
        let entries = parse_layer_entries(&stack_xml);

        let mut layers = Vec::new();
        for entry in entries.iter().rev() {
            let mut png_data = Vec::new();
            zip.by_name(&entry.src)
                .with_context(|| format!("missing {}", entry.src))?
                .read_to_end(&mut png_data)?;
            let img = image::load_from_memory(&png_data)
                .context("decode layer png")?
                .to_rgba8();
            let (lw, lh) = img.dimensions();
            let premul = straight_to_premul(img.as_raw());

            if lw == width && lh == height && entry.x == 0 && entry.y == 0 {
                let mut layer = Layer::new(width, height, &entry.name);
                layer.pixels = premul;
                layer.opacity = entry.opacity;
                layer.visible = entry.visible;
                layer.blend = entry.blend;
                layers.push(layer);
            } else {
                let mut layer = Layer::new(width, height, &entry.name);
                layer.opacity = entry.opacity;
                layer.visible = entry.visible;
                layer.blend = entry.blend;
                for y in 0..lh {
                    let dst_y = entry.y + y as i32;
                    if dst_y < 0 || dst_y >= height as i32 {
                        continue;
                    }
                    for x in 0..lw {
                        let dst_x = entry.x + x as i32;
                        if dst_x < 0 || dst_x >= width as i32 {
                            continue;
                        }
                        let si = ((y * lw + x) * 4) as usize;
                        let di = ((dst_y as u32 * width + dst_x as u32) * 4) as usize;
                        layer.pixels[di..di + 4]
                            .copy_from_slice(&premul[si..si + 4]);
                    }
                }
                layers.push(layer);
            }
        }

        anyhow::ensure!(!layers.is_empty(), "no layers found in ORA file");

        Ok(Self {
            width,
            height,
            layers,
            active_layer: 0,
            path: Some(path.to_path_buf()),
        })
    }

    pub fn add_layer(&mut self) {
        let n = self.layers.len() + 1;
        self.layers.push(Layer::new(self.width, self.height, format!("Layer {n}")));
        self.active_layer = self.layers.len() - 1;
    }

    pub fn remove_layer(&mut self, index: usize) -> bool {
        if self.layers.len() <= 1 {
            return false;
        }
        if index >= self.layers.len() {
            return false;
        }
        self.layers.remove(index);
        if self.active_layer >= self.layers.len() {
            self.active_layer = self.layers.len() - 1;
        } else if self.active_layer > index {
            self.active_layer -= 1;
        }
        true
    }

    pub fn resize_canvas(&mut self, new_w: u32, new_h: u32) {
        let copy_w = self.width.min(new_w);
        let copy_h = self.height.min(new_h);
        for layer in &mut self.layers {
            let mut new_pixels = vec![0u8; (new_w * new_h * 4) as usize];
            for y in 0..copy_h {
                let old_start = (y * self.width * 4) as usize;
                let new_start = (y * new_w * 4) as usize;
                let len = (copy_w * 4) as usize;
                new_pixels[new_start..new_start + len]
                    .copy_from_slice(&layer.pixels[old_start..old_start + len]);
            }
            layer.pixels = new_pixels;
            layer.width = new_w;
            layer.height = new_h;
        }
        self.width = new_w;
        self.height = new_h;
    }

    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.active_layer = if self.active_layer == from {
            to
        } else if from < self.active_layer && to >= self.active_layer {
            self.active_layer - 1
        } else if from > self.active_layer && to <= self.active_layer {
            self.active_layer + 1
        } else {
            self.active_layer
        };
    }
}

/// Apply brightness (delta -1..1) and contrast (factor around 1) to straight RGBA region.
pub fn adjust_brightness_contrast_straight(buf: &mut [u8], brightness: f32, contrast: f32) {
    let b = brightness.clamp(-1.0, 1.0);
    let c = contrast.max(0.01);
    for px in buf.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 {
            continue;
        }
        for ch in &mut px[..3] {
            let mut v = *ch as f32 / 255.0;
            v = (v - 0.5) * c + 0.5 + b;
            *ch = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn make_thumbnail(img: &RgbaImage, max_size: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max_size && h <= max_size {
        return img.clone();
    }
    let scale = (max_size as f64 / w as f64).min(max_size as f64 / h as f64);
    let tw = (w as f64 * scale).round().max(1.0) as u32;
    let th = (h as f64 * scale).round().max(1.0) as u32;
    image::imageops::resize(img, tw, th, image::imageops::FilterType::Triangle)
}

struct OraLayerEntry {
    name: String,
    src: String,
    opacity: f32,
    visible: bool,
    blend: BlendMode,
    x: i32,
    y: i32,
}

fn parse_image_dims(xml: &str) -> Result<(u32, u32)> {
    let w: u32 = extract_attr(xml, "w")
        .context("missing image width")?
        .parse()
        .context("invalid image width")?;
    let h: u32 = extract_attr(xml, "h")
        .context("missing image height")?
        .parse()
        .context("invalid image height")?;
    Ok((w, h))
}

fn extract_attr(tag_region: &str, attr: &str) -> Option<String> {
    let pat = format!("{attr}=\"");
    let start = tag_region.find(&pat)? + pat.len();
    let end = start + tag_region[start..].find('"')?;
    Some(tag_region[start..end].to_string())
}

fn parse_layer_entries(xml: &str) -> Vec<OraLayerEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;
    while let Some(idx) = xml[pos..].find("<layer ") {
        let start = pos + idx;
        let Some(close) = xml[start..].find("/>") else {
            break;
        };
        let tag = &xml[start..start + close + 2];

        let src = extract_attr(tag, "src").unwrap_or_default();
        let name = extract_attr(tag, "name").unwrap_or_else(|| "Layer".into());
        let opacity: f32 = extract_attr(tag, "opacity")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);
        let visible = extract_attr(tag, "visibility")
            .map(|s| s != "hidden")
            .unwrap_or(true);
        let blend = extract_attr(tag, "composite-op")
            .map(|s| BlendMode::from_ora(&s))
            .unwrap_or(BlendMode::Normal);
        let x: i32 = extract_attr(tag, "x")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let y: i32 = extract_attr(tag, "y")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        entries.push(OraLayerEntry { name, src, opacity, visible, blend, x, y });
        pos = start + close + 2;
    }
    entries
}
