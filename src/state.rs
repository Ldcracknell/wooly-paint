use crate::document::{
    composite_layers_prefix_into, Document, History, UndoTile, UNDO_TILE_SIZE,
};
use crate::palette::PaletteBook;
use crate::tools::{BrushStampCache, ToolKind};
use gtk::cairo::ImageSurface;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FLOATING_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct FloatingSelection {
    pub id: u64,
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
    pub x: f64,
    pub y: f64,
    /// Clockwise degrees (cairo convention), pivot at bitmap center.
    pub angle_deg: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub flip_h: bool,
    pub flip_v: bool,
    /// True when lifted via Move tool (cut pushed an undo entry); false when pasted.
    pub lifted_from_layer: bool,
}

impl FloatingSelection {
    pub fn new_pasted(w: i32, h: i32, data: Vec<u8>, x: f64, y: f64) -> Self {
        Self {
            id: NEXT_FLOATING_ID.fetch_add(1, Ordering::Relaxed),
            w,
            h,
            data,
            x,
            y,
            angle_deg: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            flip_h: false,
            flip_v: false,
            lifted_from_layer: false,
        }
    }

    pub fn new_lifted(w: i32, h: i32, data: Vec<u8>, x: f64, y: f64) -> Self {
        let mut f = Self::new_pasted(w, h, data, x, y);
        f.lifted_from_layer = true;
        f
    }
}

pub use crate::selection::Selection;

/// Active drag on the floating selection (handles on the marquee, not sidebar).
#[derive(Clone, Copy, Debug)]
pub enum FloatingDrag {
    Move {
        grab_off_x: f64,
        grab_off_y: f64,
    },
    Rotate {
        base_angle_deg: f64,
        start_pointer_rad: f64,
    },
    ResizeCorner {
        dragged_corner: u8,
        anchor_doc: (f64, f64),
    },
    ResizeEdge {
        edge: u8,
        anchor_doc: (f64, f64),
    },
}

/// Left vs right mouse button color slots (primary / secondary click).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ColorSlot {
    #[default]
    Left,
    Right,
}

pub struct PendingUndoSnapshot {
    pub layer_index: usize,
    pub full_pixels: Option<Vec<u8>>,
    /// Sparse 64×64 tile snapshots keyed by `(tile_x, tile_y)`.
    pub tiles: HashMap<(u32, u32), UndoTile>,
}

pub struct AppState {
    pub doc: Document,
    pub history: History,
    pub tool: ToolKind,
    /// Left mouse button / primary drawing color.
    pub fg: [u8; 4],
    /// Right mouse button / secondary drawing color.
    pub bg: [u8; 4],
    /// GDK button id for the active canvas drag (`BUTTON_PRIMARY` / `BUTTON_SECONDARY`).
    pub pointer_drag_button: u32,
    /// Which color the hue/SV picker edits.
    pub picker_target: ColorSlot,
    /// Hue for the SV plane and hue strip, 0..1.
    pub picker_hue: f64,
    pub brush_size: f64,
    /// Remembered size for non-Pixel tools when toggling to/from Pixel.
    pub saved_brush_size: f64,
    /// Remembered size for Pixel tool when toggling to/from other tools.
    pub saved_pixel_size: f64,
    pub brush_hardness: f64,
    pub fill_tolerance: u8,
    pub shape_filled: bool,
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    /// When true, draw 1×1 pixel cell lines over the canvas (document space).
    pub show_pixel_grid: bool,
    pub selection: Option<Selection>,
    pub clipboard: Option<(i32, i32, Vec<u8>)>,
    pub floating: Option<FloatingSelection>,
    pub undo_snapshot: Option<PendingUndoSnapshot>,
    pub last_doc_pos: Option<(f64, f64)>,
    pub drag_start_doc: Option<(f64, f64)>,
    /// Line / rectangle / ellipse drag: `(tool, x0, y0, x1, y1)` in document space (live preview).
    pub shape_drag_preview: Option<(ToolKind, f64, f64, f64, f64)>,
    /// Move / rotate / resize via on-canvas handles (Move tool + floating).
    pub floating_drag: Option<FloatingDrag>,
    pub modified: bool,
    pub tool_keybinds: Vec<(ToolKind, Option<char>)>,
    /// Most recently used foreground colors (straight RGBA), newest first; at most 4 kept.
    pub recent_colors: Vec<[u8; 4]>,
    /// Named palettes (sidebar); first entry is the built-in default.
    pub palette_book: PaletteBook,
    /// Recently opened documents (paths), newest first; at most 5 kept.
    pub recent_files: Vec<PathBuf>,
    /// Bumped when layer pixels, stack, visibility, opacity, or canvas size change (not selection/pan/zoom).
    pub document_visual_revision: u64,
    /// Bumped when layers other than the active layer (or stack structure/visibility/opacity/blend/size) change.
    pub layers_stack_revision: u64,
    /// Cached full-document composite (premultiplied RGBA), valid when `composite_cache_at_revision == document_visual_revision`.
    pub composite_cache_premul: Vec<u8>,
    /// Cairo `ImageSurface` for painting the flattened document (BGRA premul); pixels updated from `composite_cache_premul`.
    pub composite_cache_surface: Option<ImageSurface>,
    pub composite_cache_at_revision: u64,
    /// Cached Cairo `ImageSurface` for the current `FloatingSelection`, keyed by `FloatingSelection::id`.
    pub floating_surface_cache: Option<ImageSurface>,
    pub floating_surface_id: Option<u64>,
    /// Persistent brush falloff + subpixel mask cache across drag events and strokes.
    pub brush_stamp_cache: BrushStampCache,
    /// While true, incremental dirty-rect compositing is used during brush/pixel/eraser strokes.
    pub brush_stroke_in_progress: bool,
    /// Distance from the latest stroke sample to the next brush dab, carried across drag events.
    pub stroke_next_dab_distance: f64,
    /// Previous raw document-space brush sample, used to smooth fast strokes into curves.
    pub stroke_smooth_prev_doc: Option<(f64, f64)>,
    pub stroke_smooth_started: bool,
    /// During brush/pixel/eraser stroke: only paint inside this selection (captured at press; `None` = no clip).
    pub stroke_paint_clip: Option<Selection>,
    /// Reusable cached premul RGBA of layers strictly below `active_layer`.
    /// Empty when no visible layers exist below `active_layer`.
    pub below_cache_buf: Vec<u8>,
    pub below_cache_has_content: bool,
    pub below_cache_revision: u64,
    pub below_cache_active_layer: usize,
    pub below_cache_doc_wh: (u32, u32),
    /// Document-space pixels touched since the last canvas composite update.
    pub stroke_dirty_rect: Option<(i32, i32, i32, i32)>,
}

impl AppState {
    /// Paint color for the active pointer drag (left → [`Self::fg`], right → [`Self::bg`]).
    pub fn active_paint_color(&self) -> [u8; 4] {
        if self.pointer_drag_button == 3 {
            self.bg
        } else {
            self.fg
        }
    }

    pub fn default_tool_keybinds() -> Vec<(ToolKind, Option<char>)> {
        vec![
            (ToolKind::Brush, Some('b')),
            (ToolKind::Pixel, Some('p')),
            (ToolKind::Eraser, Some('e')),
            (ToolKind::Eyedropper, Some('k')),
            (ToolKind::Fill, Some('f')),
            (ToolKind::Line, Some('l')),
            (ToolKind::Rect, None),
            (ToolKind::Ellipse, None),
            (ToolKind::SelectRect, Some('s')),
            (ToolKind::MagicSelect, Some('w')),
            (ToolKind::Move, Some('m')),
            (ToolKind::Hand, Some('h')),
        ]
    }

    pub fn new() -> Self {
        Self {
            doc: Document::new(800, 600),
            history: History::new(),
            tool: ToolKind::Brush,
            fg: [0, 0, 0, 255],
            bg: [255, 255, 255, 255],
            pointer_drag_button: 1,
            picker_target: ColorSlot::Left,
            picker_hue: 0.0,
            brush_size: 8.0,
            saved_brush_size: 8.0,
            saved_pixel_size: 1.0,
            brush_hardness: 0.85,
            fill_tolerance: 32,
            shape_filled: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            show_pixel_grid: false,
            selection: None,
            clipboard: None,
            floating: None,
            undo_snapshot: None,
            last_doc_pos: None,
            drag_start_doc: None,
            shape_drag_preview: None,
            floating_drag: None,
            modified: false,
            tool_keybinds: Self::default_tool_keybinds(),
            recent_colors: Vec::new(),
            palette_book: PaletteBook::new_builtin_only(),
            recent_files: Vec::new(),
            document_visual_revision: 0,
            layers_stack_revision: 0,
            composite_cache_premul: Vec::new(),
            composite_cache_surface: None,
            composite_cache_at_revision: u64::MAX,
            floating_surface_cache: None,
            floating_surface_id: None,
            brush_stamp_cache: BrushStampCache::new(),
            brush_stroke_in_progress: false,
            stroke_next_dab_distance: 0.0,
            stroke_smooth_prev_doc: None,
            stroke_smooth_started: false,
            stroke_paint_clip: None,
            below_cache_buf: Vec::new(),
            below_cache_has_content: false,
            below_cache_revision: u64::MAX,
            below_cache_active_layer: usize::MAX,
            below_cache_doc_wh: (0, 0),
            stroke_dirty_rect: None,
        }
    }

    /// Ensure the cached composite of layers strictly below `active_layer` is up-to-date.
    /// Reuses the buffer across strokes and skips allocation/compositing entirely when no visible
    /// layers exist below `active_layer` (e.g. single-layer or bottom-layer painting).
    pub fn ensure_stroke_composite_below(&mut self) {
        let w = self.doc.width;
        let h = self.doc.height;
        let active = self.doc.active_layer;

        if self.below_cache_revision == self.layers_stack_revision
            && self.below_cache_active_layer == active
            && self.below_cache_doc_wh == (w, h)
        {
            return;
        }

        let any_visible_below = self
            .doc
            .layers
            .iter()
            .take(active)
            .any(|l| l.visible && l.opacity > 0.0);

        if !any_visible_below {
            self.below_cache_has_content = false;
        } else {
            let len = (w * h * 4) as usize;
            self.below_cache_buf.resize(len, 0);
            composite_layers_prefix_into(&mut self.below_cache_buf, w, h, &self.doc.layers, active);
            self.below_cache_has_content = true;
        }
        self.below_cache_revision = self.layers_stack_revision;
        self.below_cache_active_layer = active;
        self.below_cache_doc_wh = (w, h);
    }

    pub fn below_cache_valid(&self) -> bool {
        let (dw, dh) = (self.doc.width, self.doc.height);
        self.below_cache_revision == self.layers_stack_revision
            && self.below_cache_active_layer == self.doc.active_layer
            && self.below_cache_doc_wh == (dw, dh)
            && (!self.below_cache_has_content || self.below_cache_buf.len() == (dw * dh * 4) as usize)
    }

    /// Invalidate composite cache (call after any change that affects flattened pixels).
    pub fn bump_document_revision(&mut self) {
        self.document_visual_revision = self.document_visual_revision.wrapping_add(1);
    }

    /// Invalidate both composite cache AND the cached "layers below active" composite
    /// (call on layer stack edits, visibility/opacity/blend changes, undo/redo, resize, open/new).
    pub fn bump_stack_revision(&mut self) {
        self.document_visual_revision = self.document_visual_revision.wrapping_add(1);
        self.layers_stack_revision = self.layers_stack_revision.wrapping_add(1);
    }

    pub fn add_stroke_dirty_rect(&mut self, rect: Option<(i32, i32, i32, i32)>) {
        let Some((x, y, w, h)) = rect else {
            return;
        };
        if w <= 0 || h <= 0 {
            return;
        }
        let incoming = (x, y, w, h);
        self.stroke_dirty_rect = Some(match self.stroke_dirty_rect {
            None => incoming,
            Some((ax, ay, aw, ah)) => {
                let x0 = ax.min(x);
                let y0 = ay.min(y);
                let x1 = (ax + aw).max(x + w);
                let y1 = (ay + ah).max(y + h);
                (x0, y0, x1 - x0, y1 - y0)
            }
        });
    }

    pub fn take_stroke_dirty_rect(&mut self) -> Option<(i32, i32, i32, i32)> {
        self.stroke_dirty_rect.take()
    }

    /// Drop GPU-adjacent composite caches on application shutdown so heap blocks are freed before exit.
    pub fn release_drawing_caches(&mut self) {
        self.below_cache_buf.clear();
        self.below_cache_buf.shrink_to_fit();
        self.below_cache_has_content = false;
        self.below_cache_revision = u64::MAX;
        self.stroke_dirty_rect = None;
        self.composite_cache_surface = None;
        self.composite_cache_premul.clear();
        self.composite_cache_premul.shrink_to_fit();
        self.composite_cache_at_revision = u64::MAX;
        self.floating_surface_cache = None;
        self.floating_surface_id = None;
    }

    pub fn widget_to_doc(&self, wx: f64, wy: f64) -> (f64, f64) {
        ((wx - self.pan_x) / self.zoom, (wy - self.pan_y) / self.zoom)
    }

    pub fn begin_full_undo(&mut self) {
        if let Some(layer) = self.doc.active_layer_ref() {
            self.undo_snapshot = Some(PendingUndoSnapshot {
                layer_index: self.doc.active_layer,
                full_pixels: Some(layer.pixels.clone()),
                tiles: HashMap::new(),
            });
        }
    }

    pub fn begin_stroke_undo(&mut self) {
        if self.doc.active_layer_ref().is_some() {
            self.undo_snapshot = Some(PendingUndoSnapshot {
                layer_index: self.doc.active_layer,
                full_pixels: None,
                tiles: HashMap::new(),
            });
        }
    }

    /// Snapshot only untouched 64×64 tiles overlapping `rect` (O(newly-touched tiles), zero reallocation).
    pub fn capture_undo_rect(&mut self, rect: Option<(i32, i32, i32, i32)>) {
        let Some(rect) = rect else {
            return;
        };
        let Some(snapshot) = self.undo_snapshot.as_mut() else {
            return;
        };
        if snapshot.full_pixels.is_some() || snapshot.layer_index != self.doc.active_layer {
            return;
        }
        let Some(layer) = self.doc.layers.get(snapshot.layer_index) else {
            return;
        };
        let Some((rx0, ry0, rw, rh)) = clip_rect_to_layer(layer.width, layer.height, rect) else {
            return;
        };
        let tx0 = (rx0 / UNDO_TILE_SIZE) as u32;
        let ty0 = (ry0 / UNDO_TILE_SIZE) as u32;
        let tx1 = ((rx0 + rw - 1) / UNDO_TILE_SIZE) as u32;
        let ty1 = ((ry0 + rh - 1) / UNDO_TILE_SIZE) as u32;
        let lw = layer.width as i32;
        let lh = layer.height as i32;

        for ty in ty0..=ty1 {
            let y = ty as i32 * UNDO_TILE_SIZE;
            let th = (lh - y).min(UNDO_TILE_SIZE);
            for tx in tx0..=tx1 {
                snapshot.tiles.entry((tx, ty)).or_insert_with(|| {
                    let x = tx as i32 * UNDO_TILE_SIZE;
                    let tw = (lw - x).min(UNDO_TILE_SIZE);
                    let pixels = copy_layer_rect(layer, (x, y, tw, th));
                    UndoTile {
                        x,
                        y,
                        w: tw,
                        h: th,
                        pixels,
                    }
                });
            }
        }
    }

    /// Commit pending undo snapshot. When `bump_revision` is `false`, caller is responsible for keeping
    /// the composite cache synchronized (used on brush stroke end after incremental dirty-rect updates).
    pub fn commit_stroke_undo_with_revision(&mut self, bump_revision: bool) -> bool {
        let Some(snapshot) = self.undo_snapshot.take() else {
            return false;
        };
        let Some(layer) = self.doc.layers.get(snapshot.layer_index) else {
            return false;
        };
        if let Some(before) = snapshot.full_pixels {
            if layer.pixels != before {
                self.history.commit_change(snapshot.layer_index, before);
                self.modified = true;
                if bump_revision {
                    self.bump_document_revision();
                }
                return true;
            }
            return false;
        }
        let changed_tiles: Vec<UndoTile> = snapshot
            .tiles
            .into_values()
            .filter(|t| layer_rect_differs(layer, (t.x, t.y, t.w, t.h), &t.pixels))
            .collect();
        if !changed_tiles.is_empty() {
            self.history
                .commit_tiles_change(snapshot.layer_index, changed_tiles);
            self.modified = true;
            if bump_revision {
                self.bump_document_revision();
            }
            return true;
        }
        false
    }

    pub fn commit_stroke_undo(&mut self) -> bool {
        self.commit_stroke_undo_with_revision(true)
    }

    pub fn undo(&mut self) -> bool {
        if self.history.undo(&mut self.doc) {
            self.modified = true;
            self.bump_stack_revision();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if self.history.redo(&mut self.doc) {
            self.modified = true;
            self.bump_stack_revision();
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

fn clip_rect_to_layer(
    width: u32,
    height: u32,
    rect: (i32, i32, i32, i32),
) -> Option<(i32, i32, i32, i32)> {
    let (x, y, w, h) = rect;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(width as i32);
    let y1 = (y + h).min(height as i32);
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

fn copy_layer_rect(layer: &crate::document::Layer, rect: (i32, i32, i32, i32)) -> Vec<u8> {
    let (x, y, w, h) = rect;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for row in 0..h {
        let src = layer.idx(x as u32, (y + row) as u32);
        let dst = (row * w * 4) as usize;
        let len = (w * 4) as usize;
        out[dst..dst + len].copy_from_slice(&layer.pixels[src..src + len]);
    }
    out
}

fn layer_rect_differs(
    layer: &crate::document::Layer,
    rect: (i32, i32, i32, i32),
    before: &[u8],
) -> bool {
    let (x, y, w, h) = rect;
    if before.len() < (w * h * 4) as usize {
        return false;
    }
    for row in 0..h {
        let src = layer.idx(x as u32, (y + row) as u32);
        let old = (row * w * 4) as usize;
        let len = (w * 4) as usize;
        if layer.pixels[src..src + len] != before[old..old + len] {
            return true;
        }
    }
    false
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
