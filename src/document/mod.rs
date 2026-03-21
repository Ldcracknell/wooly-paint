mod composite;
mod history;
pub mod layer;

pub use composite::{composite_layers, premul_to_straight_rgba, straight_to_premul};
pub use history::History;
pub use layer::{BlendMode, Layer};

use anyhow::{Context, Result};
use image::{ImageBuffer, RgbaImage};
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
        let mut layers = Vec::new();
        layers.push(Layer::new(width, height, "Background"));
        Self {
            width,
            height,
            layers,
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
