use super::Document;
use std::collections::VecDeque;

/// Maximum undo steps retained (oldest dropped). Bounds memory from unbounded layer buffer copies.
const MAX_UNDO_ENTRIES: usize = 64;

/// Tile edge in document pixels for sparse stroke undo snapshots.
pub const UNDO_TILE_SIZE: i32 = 64;

#[derive(Clone)]
pub struct UndoTile {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub pixels: Vec<u8>,
}

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
    Tiles {
        layer_index: usize,
        tiles: Vec<UndoTile>,
    },
}

impl LayerUndoEntry {
    fn layer_index(&self) -> usize {
        match self {
            Self::Full { layer_index, .. }
            | Self::Rect { layer_index, .. }
            | Self::Tiles { layer_index, .. } => *layer_index,
        }
    }

    fn layer_index_mut(&mut self) -> &mut usize {
        match self {
            Self::Full { layer_index, .. }
            | Self::Rect { layer_index, .. }
            | Self::Tiles { layer_index, .. } => layer_index,
        }
    }
}

pub struct History {
    undo: VecDeque<LayerUndoEntry>,
    redo: Vec<LayerUndoEntry>,
}

#[allow(dead_code)]
impl History {
    pub fn new() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn push_undo(&mut self, entry: LayerUndoEntry) {
        if self.undo.len() >= MAX_UNDO_ENTRIES {
            self.undo.pop_front();
        }
        self.undo.push_back(entry);
        self.redo.clear();
    }

    /// `before` is the layer pixel buffer before the edit; call after the edit is applied.
    pub fn commit_change(&mut self, layer_index: usize, before: Vec<u8>) {
        self.push_undo(LayerUndoEntry::Full {
            layer_index,
            pixels: before,
        });
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
        self.push_undo(LayerUndoEntry::Rect {
            layer_index,
            x,
            y,
            w,
            h,
            pixels: before,
        });
    }

    /// Commit sparse tile snapshots captured before a stroke.
    pub fn commit_tiles_change(&mut self, layer_index: usize, tiles: Vec<UndoTile>) {
        if tiles.is_empty() {
            return;
        }
        self.push_undo(LayerUndoEntry::Tiles { layer_index, tiles });
    }

    /// Keep layer indices valid after `Document::move_layer(from, to)`.
    pub fn remap_layer_move(&mut self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let remap = |idx: &mut usize| {
            *idx = if *idx == from {
                to
            } else if from < *idx && to >= *idx {
                *idx - 1
            } else if from > *idx && to <= *idx {
                *idx + 1
            } else {
                *idx
            };
        };
        for entry in &mut self.undo {
            remap(entry.layer_index_mut());
        }
        for entry in &mut self.redo {
            remap(entry.layer_index_mut());
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        let Some(mut entry) = self.undo.pop_back() else {
            return false;
        };
        let idx = entry.layer_index();
        let Some(layer) = doc.layers.get_mut(idx) else {
            return false;
        };
        apply_swap(layer, &mut entry);
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
        apply_swap(layer, &mut entry);
        self.undo.push_back(entry);
        true
    }
}

fn apply_swap(layer: &mut crate::document::Layer, entry: &mut LayerUndoEntry) {
    match entry {
        LayerUndoEntry::Full { pixels, .. } => {
            std::mem::swap(&mut layer.pixels, pixels);
        }
        LayerUndoEntry::Rect {
            x, y, w, h, pixels, ..
        } => swap_rect_pixels(layer, *x, *y, *w, *h, pixels),
        LayerUndoEntry::Tiles { tiles, .. } => {
            for tile in tiles.iter_mut() {
                swap_rect_pixels(layer, tile.x, tile.y, tile.w, tile.h, &mut tile.pixels);
            }
        }
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
    let lw = layer.width as i32;
    let lh = layer.height as i32;
    let col_start = (-x).max(0);
    let col_end = (lw - x).min(w);
    let row_start = (-y).max(0);
    let row_end = (lh - y).min(h);
    if col_end <= col_start || row_end <= row_start {
        return;
    }
    let span_bytes = ((col_end - col_start) * 4) as usize;
    for row in row_start..row_end {
        let sy = (y + row) as u32;
        let sx = (x + col_start) as u32;
        let li = layer.idx(sx, sy);
        let pi = ((row * w + col_start) * 4) as usize;
        layer.pixels[li..li + span_bytes].swap_with_slice(&mut pixels[pi..pi + span_bytes]);
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn tile_undo_redo_round_trip() {
        let mut doc = Document::new(128, 128);
        let orig = doc.layers[0].pixels.clone();
        let tile_before = vec![255u8; 64 * 64 * 4];
        doc.layers[0].pixels[0..4].copy_from_slice(&[10, 20, 30, 255]);
        let edited = doc.layers[0].pixels.clone();

        let mut history = History::new();
        history.commit_tiles_change(
            0,
            vec![UndoTile {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
                pixels: tile_before,
            }],
        );
        assert!(history.undo(&mut doc));
        assert_eq!(doc.layers[0].pixels, orig);
        assert!(history.redo(&mut doc));
        assert_eq!(doc.layers[0].pixels, edited);
    }

    #[test]
    fn remap_layer_move_keeps_undo_targeting_same_layer() {
        let mut doc = Document::new(8, 8);
        doc.add_layer();
        let orig_top = doc.layers[1].pixels.clone();
        doc.layers[1].pixels[0..4].copy_from_slice(&[50, 60, 70, 255]);
        let mut history = History::new();
        history.commit_change(1, orig_top.clone());

        doc.move_layer(1, 0);
        history.remap_layer_move(1, 0);

        assert!(history.undo(&mut doc));
        assert_eq!(doc.layers[0].pixels, orig_top);
        assert_eq!(doc.layers[1].pixels[0..4], [255, 255, 255, 255]);
    }
}
