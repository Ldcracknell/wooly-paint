//! Persistent app preferences (color theme, tool keybinds) under the XDG config directory.

use crate::palette::{NamedPalette, PaletteBook};
use crate::state::AppState;
use crate::tools::ToolKind;
use libadwaita::ColorScheme;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Max paths kept in File → Recent Files and in settings.json.
pub const MAX_RECENT_FILES: usize = 5;

const CONFIG_DIR: &str = "wooly-paint";
const SETTINGS_FILE: &str = "settings.json";

#[derive(Serialize, Deserialize, Default)]
struct FileSettings {
    #[serde(default)]
    color_scheme: String,
    #[serde(default)]
    tool_keybinds: Vec<StoredBind>,
    #[serde(default)]
    recent_files: Vec<String>,
    #[serde(default)]
    palettes: Vec<NamedPalette>,
    #[serde(default)]
    active_palette: usize,
    #[serde(default)]
    show_pixel_grid: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredBind {
    tool: String,
    #[serde(default)]
    key: Option<String>,
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut h = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            h.push(".config");
            h
        });
    base.join(CONFIG_DIR).join(SETTINGS_FILE)
}

fn tool_to_name(t: ToolKind) -> &'static str {
    t.display_name()
}

fn tool_from_name(s: &str) -> Option<ToolKind> {
    match s.trim() {
        "Brush" => Some(ToolKind::Brush),
        "Pixel" => Some(ToolKind::Pixel),
        "Eraser" => Some(ToolKind::Eraser),
        "Eyedropper" => Some(ToolKind::Eyedropper),
        "Fill" => Some(ToolKind::Fill),
        "Line" => Some(ToolKind::Line),
        "Rectangle" => Some(ToolKind::Rect),
        "Ellipse" => Some(ToolKind::Ellipse),
        "Select" => Some(ToolKind::SelectRect),
        "Magic select" => Some(ToolKind::MagicSelect),
        "Move" => Some(ToolKind::Move),
        "Hand" => Some(ToolKind::Hand),
        _ => None,
    }
}

fn normalized_theme(s: &str) -> &'static str {
    match s {
        "light" => "light",
        "dark" => "dark",
        _ => "default",
    }
}

/// Read only the stored appearance value (`"default"`, `"light"`, or `"dark"`) without touching [`AppState`].
/// Used before `GtkApplication` exists so libadwaita can apply the scheme early.
pub(crate) fn saved_color_scheme_menu_value() -> &'static str {
    let path = config_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return "default";
    };
    let Ok(parsed) = serde_json::from_slice::<FileSettings>(&bytes) else {
        return "default";
    };
    normalized_theme(parsed.color_scheme.trim())
}

fn merge_keybinds(stored: &[StoredBind]) -> Vec<(ToolKind, Option<char>)> {
    let mut out = AppState::default_tool_keybinds();
    if stored.is_empty() {
        return out;
    }
    for b in stored {
        let Some(tool) = tool_from_name(&b.tool) else {
            continue;
        };
        let key = b
            .key
            .as_ref()
            .and_then(|s| s.chars().next())
            .map(|c| c.to_ascii_lowercase())
            .filter(|c| !c.is_control() && !c.is_whitespace());
        if let Some(idx) = out.iter().position(|(t, _)| *t == tool) {
            out[idx].1 = key;
        }
    }
    let mut seen = std::collections::HashSet::new();
    for i in 0..out.len() {
        if let Some(c) = out[i].1 {
            if !seen.insert(c) {
                out[i].1 = None;
            }
        }
    }
    out
}

fn color_scheme_to_storage(scheme: ColorScheme) -> &'static str {
    match scheme {
        ColorScheme::ForceLight | ColorScheme::PreferLight => "light",
        ColorScheme::ForceDark | ColorScheme::PreferDark => "dark",
        ColorScheme::Default => "default",
        _ => "default",
    }
}

/// Load preferences into `state` and return the color theme menu value (`"default"`, `"light"`, or `"dark"`).
pub fn load_into(state: &mut AppState) -> &'static str {
    let path = config_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return "default";
    };
    let parsed: FileSettings = match serde_json::from_slice(&bytes) {
        Ok(p) => p,
        Err(_) => return "default",
    };
    if !parsed.tool_keybinds.is_empty() {
        state.tool_keybinds = merge_keybinds(&parsed.tool_keybinds);
    }
    if !parsed.palettes.is_empty() {
        state.palette_book = PaletteBook::from_loaded(parsed.palettes, parsed.active_palette);
    }
    if state.palette_book.merge_missing_builtin_presets() {
        persist(state);
    }
    state.show_pixel_grid = parsed.show_pixel_grid;
    state.recent_files = parsed
        .recent_files
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .take(MAX_RECENT_FILES)
        .collect();
    normalized_theme(parsed.color_scheme.trim())
}

/// Write current preferences to disk (best-effort; ignores I/O errors).
pub fn persist(state: &AppState) {
    let scheme = libadwaita::StyleManager::default().color_scheme();
    let theme = color_scheme_to_storage(scheme);
    let binds: Vec<StoredBind> = state
        .tool_keybinds
        .iter()
        .map(|(t, k)| StoredBind {
            tool: tool_to_name(*t).to_string(),
            key: k.map(|c| c.to_string()),
        })
        .collect();
    let recent_files: Vec<String> = state
        .recent_files
        .iter()
        .filter_map(|p| p.to_str().map(str::to_owned))
        .collect();
    let data = FileSettings {
        color_scheme: theme.to_string(),
        tool_keybinds: binds,
        recent_files,
        palettes: state.palette_book.entries.clone(),
        active_palette: state.palette_book.active,
        show_pixel_grid: state.show_pixel_grid,
    };
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(path, json);
    }
}

/// Move `path` to the front of the recent list (deduped), cap at [`MAX_RECENT_FILES`], persist.
pub fn record_recent_open(state: &mut AppState, path: PathBuf) {
    state.recent_files.retain(|p| p != &path);
    state.recent_files.insert(0, path);
    state.recent_files.truncate(MAX_RECENT_FILES);
    persist(state);
}
