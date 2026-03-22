use super::Document;

/// Maximum undo steps retained (oldest dropped). Bounds memory from unbounded layer buffer copies.
const MAX_UNDO_ENTRIES: usize = 64;

#[derive(Clone)]
pub struct LayerUndoEntry {
    pub layer_index: usize,
    pub pixels: Vec<u8>,
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
        self.undo.push(LayerUndoEntry {
            layer_index,
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
        let idx = entry.layer_index;
        let Some(layer) = doc.layers.get_mut(idx) else {
            return false;
        };
        std::mem::swap(&mut layer.pixels, &mut entry.pixels);
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(mut entry) = self.redo.pop() else {
            return false;
        };
        let idx = entry.layer_index;
        let Some(layer) = doc.layers.get_mut(idx) else {
            return false;
        };
        std::mem::swap(&mut layer.pixels, &mut entry.pixels);
        self.undo.push(entry);
        true
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}
