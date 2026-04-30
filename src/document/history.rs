use super::Document;

/// Maximum undo steps retained (oldest dropped). Bounds memory from unbounded layer buffer copies.
const MAX_UNDO_ENTRIES: usize = 64;

#[derive(Clone)]
pub enum LayerUndoEntry {
    Full {
        layer_index: usize,
        pixels: Vec<u8>,
    },
    Rect {
        layer_index: usize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        pixels: Vec<u8>,
    },
}

impl LayerUndoEntry {
    fn layer_index(&self) -> usize {
        match self {
            Self::Full { layer_index, .. } | Self::Rect { layer_index, .. } => *layer_index,
        }
    }
}

pub struct History {
    undo: Vec<LayerUndoEntry>,
    redo: Vec<LayerUndoEntry>,
}

#[allow(dead_code)]
impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// `before` is the layer pixel buffer before the edit; call after the edit is applied.
    pub fn commit_change(&mut self, layer_index: usize, before: Vec<u8>) {
        if self.undo.len() >= MAX_UNDO_ENTRIES {
            self.undo.remove(0);
        }
        self.undo.push(LayerUndoEntry::Full {
            layer_index,
            pixels: before,
        });
        self.redo.clear();
    }

    /// `before` is a tight row-major `w * h * 4` premultiplied buffer for document rect `(x,y,w,h)`.
    pub fn commit_rect_change(
        &mut self,
        layer_index: usize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        before: Vec<u8>,
    ) {
        if w <= 0 || h <= 0 || before.is_empty() {
            return;
        }
        if self.undo.len() >= MAX_UNDO_ENTRIES {
            self.undo.remove(0);
        }
        self.undo.push(LayerUndoEntry::Rect {
            layer_index,
            x,
            y,
            w,
            h,
            pixels: before,
        });
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        let Some(mut entry) = self.undo.pop() else {
            return false;
        };
        let idx = entry.layer_index();
        let Some(layer) = doc.layers.get_mut(idx) else {
            return false;
        };
        match &mut entry {
            LayerUndoEntry::Full { pixels, .. } => {
                std::mem::swap(&mut layer.pixels, pixels);
            }
            LayerUndoEntry::Rect {
                x, y, w, h, pixels, ..
            } => swap_rect_pixels(layer, *x, *y, *w, *h, pixels),
        }
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(mut entry) = self.redo.pop() else {
            return false;
        };
        let idx = entry.layer_index();
        let Some(layer) = doc.layers.get_mut(idx) else {
            return false;
        };
        match &mut entry {
            LayerUndoEntry::Full { pixels, .. } => {
                std::mem::swap(&mut layer.pixels, pixels);
            }
            LayerUndoEntry::Rect {
                x, y, w, h, pixels, ..
            } => swap_rect_pixels(layer, *x, *y, *w, *h, pixels),
        }
        self.undo.push(entry);
        true
    }
}

fn swap_rect_pixels(
    layer: &mut crate::document::Layer,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    pixels: &mut [u8],
) {
    if w <= 0 || h <= 0 || pixels.len() < (w * h * 4) as usize {
        return;
    }
    for row in 0..h {
        for col in 0..w {
            let sx = x + col;
            let sy = y + row;
            if sx < 0 || sy < 0 || sx >= layer.width as i32 || sy >= layer.height as i32 {
                continue;
            }
            let pi = ((row * w + col) * 4) as usize;
            let li = layer.idx(sx as u32, sy as u32);
            for c in 0..4 {
                std::mem::swap(&mut layer.pixels[li + c], &mut pixels[pi + c]);
            }
        }
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}
