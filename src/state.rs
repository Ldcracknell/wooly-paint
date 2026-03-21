use crate::document::{Document, History};
use crate::tools::ToolKind;

#[derive(Clone)]
pub struct FloatingSelection {
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
    pub x: f64,
    pub y: f64,
}

pub struct AppState {
    pub doc: Document,
    pub history: History,
    pub tool: ToolKind,
    pub fg: [u8; 4],
    pub brush_size: f64,
    pub brush_hardness: f64,
    pub fill_tolerance: u8,
    pub shape_filled: bool,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub selection: Option<(i32, i32, i32, i32)>,
    pub clipboard: Option<(i32, i32, Vec<u8>)>,
    pub floating: Option<FloatingSelection>,
    pub undo_snapshot: Option<(usize, Vec<u8>)>,
    pub last_doc_pos: Option<(f64, f64)>,
    pub drag_start_doc: Option<(f64, f64)>,
    /// When dragging a floating selection: `(pointer_doc_x - float_x, pointer_doc_y - float_y)`.
    pub move_grab_doc: Option<(f64, f64)>,
    pub modified: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            doc: Document::new(800, 600),
            history: History::new(),
            tool: ToolKind::Brush,
            fg: [0, 0, 0, 255],
            brush_size: 8.0,
            brush_hardness: 0.85,
            fill_tolerance: 32,
            shape_filled: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            selection: None,
            clipboard: None,
            floating: None,
            undo_snapshot: None,
            last_doc_pos: None,
            drag_start_doc: None,
            move_grab_doc: None,
            modified: false,
        }
    }

    pub fn widget_to_doc(&self, wx: f64, wy: f64) -> (f64, f64) {
        ((wx - self.pan_x) / self.zoom, (wy - self.pan_y) / self.zoom)
    }

    pub fn begin_stroke_undo(&mut self) {
        if let Some(layer) = self.doc.active_layer_ref() {
            self.undo_snapshot = Some((self.doc.active_layer, layer.pixels.clone()));
        }
    }

    pub fn commit_stroke_undo(&mut self) {
        if let Some((idx, before)) = self.undo_snapshot.take() {
            if let Some(layer) = self.doc.layers.get(idx) {
                if layer.pixels != before {
                    self.history.commit_change(idx, before);
                    self.modified = true;
                }
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.history.undo(&mut self.doc) {
            self.modified = true;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if self.history.redo(&mut self.doc) {
            self.modified = true;
            true
        } else {
            false
        }
    }

    pub fn normalize_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> (i32, i32, i32, i32) {
        let min_x = x0.min(x1).floor() as i32;
        let min_y = y0.min(y1).floor() as i32;
        let max_x = x0.max(x1).ceil() as i32;
        let max_y = y0.max(y1).ceil() as i32;
        let w = (max_x - min_x).max(1);
        let h = (max_y - min_y).max(1);
        (min_x, min_y, w, h)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
