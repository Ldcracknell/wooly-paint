//! Named color palettes, hex import/export (Lospec-style `#RRGGBB` lines), and persistence helpers.

use serde::{Deserialize, Serialize};

/// Built-in sidebar palette name (always kept at index0; not deletable).
pub const BUILTIN_PALETTE_NAME: &str = "Default";

/// Hard cap per palette to keep the UI responsive.
pub const MAX_COLORS_PER_PALETTE: usize = 512;

/// Straight RGBA presets matching the original sidebar defaults.
pub const BUILTIN_SWATCHES: &[[u8; 4]] = &[
    [0, 0, 0, 255],
    [255, 255, 255, 255],
    [255, 0, 0, 255],
    [255, 128, 0, 255],
    [255, 200, 0, 255],
    [0, 160, 0, 255],
    [0, 220, 220, 255],
    [0, 100, 255, 255],
    [160, 0, 255, 255],
    [255, 0, 200, 255],
    [64, 64, 64, 255],
    [128, 128, 128, 255],
    [192, 192, 192, 255],
    [139, 90, 43, 255],
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamedPalette {
    pub name: String,
    pub colors: Vec<[u8; 4]>,
}

impl NamedPalette {
    pub fn new(name: impl Into<String>, colors: Vec<[u8; 4]>) -> Self {
        Self {
            name: name.into(),
            colors,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteBook {
    pub entries: Vec<NamedPalette>,
    pub active: usize,
}

impl PaletteBook {
    pub fn new_builtin_only() -> Self {
        Self {
            entries: vec![NamedPalette::new(
                BUILTIN_PALETTE_NAME,
                BUILTIN_SWATCHES.to_vec(),
            )],
            active: 0,
        }
    }

    /// Restore from settings; falls back to built-in if data is unusable.
    pub fn from_loaded(mut entries: Vec<NamedPalette>, active: usize) -> Self {
        entries.retain(|e| !e.name.is_empty() && !e.colors.is_empty());
        for e in &mut entries {
            e.colors.truncate(MAX_COLORS_PER_PALETTE);
        }
        if entries.is_empty() {
            return Self::new_builtin_only();
        }
        let active = active.min(entries.len() - 1);
        Self { entries, active }
    }

    pub fn active_palette(&self) -> &NamedPalette {
        &self.entries[self.active]
    }

    pub fn active_colors(&self) -> &[[u8; 4]] {
        &self.entries[self.active].colors
    }

    pub fn clamp_active(&mut self) {
        if self.entries.is_empty() {
            *self = Self::new_builtin_only();
            return;
        }
        self.active = self.active.min(self.entries.len() - 1);
    }

    /// Insert after import or duplicate. `name` is trimmed; empty becomes `"Imported"`.
    pub fn push_palette(&mut self, name: impl AsRef<str>, colors: Vec<[u8; 4]>) {
        let name = name.as_ref().trim();
        let name = if name.is_empty() {
            "Imported".to_string()
        } else {
            name.to_string()
        };
        self.entries.push(NamedPalette::new(name, colors));
        self.active = self.entries.len() - 1;
        self.clamp_active();
    }

    pub fn duplicate_entry(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        let base = self.entries[index].name.clone();
        let colors = self.entries[index].colors.clone();
        let name = format!("Copy of {base}");
        self.entries.push(NamedPalette::new(name, colors));
        self.active = self.entries.len() - 1;
        true
    }

    pub fn new_empty_swatch(&mut self) {
        self.entries
            .push(NamedPalette::new("New palette", vec![[0, 0, 0, 255]]));
        self.active = self.entries.len() - 1;
    }

    /// Remove palette at index. Index 0 (built-in slot) cannot be removed.
    pub fn remove_at(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.entries.len() || self.entries.len() <= 1 {
            return false;
        }
        self.entries.remove(index);
        if self.active >= self.entries.len() {
            self.active = self.entries.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }
        true
    }

    pub fn rename(&mut self, index: usize, new_name: &str) -> bool {
        let new_name = new_name.trim();
        if new_name.is_empty() || index >= self.entries.len() {
            return false;
        }
        self.entries[index].name = new_name.to_string();
        true
    }
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + (b - b'a')),
        b'A'..=b'F' => Some(10 + (b - b'A')),
        _ => None,
    }
}

fn parse_hex_nibble_pair(hi: u8, lo: u8) -> Option<u8> {
    Some(hex_value(hi)? << 4 | hex_value(lo)?)
}

/// Expand `#RGB` to `[r,g,b]`.
fn rgb12_to_rgb24(r: u8, g: u8, b: u8) -> [u8; 3] {
    [r << 4 | r, g << 4 | g, b << 4 | b]
}

/// Parse one `#`-prefixed run starting at `bytes[i] == b'#'`. Returns `(RGBA, index after token)`.
fn parse_hash_token(bytes: &[u8], start: usize) -> Option<([u8; 4], usize)> {
    if bytes.get(start) != Some(&b'#') {
        return None;
    }
    let mut j = start + 1;
    while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
        j += 1;
    }
    let len = j - (start + 1);
    let slice = &bytes[start + 1..j];
    let rgba = match len {
        3 if slice.len() == 3 => {
            let r = hex_value(slice[0])?;
            let g = hex_value(slice[1])?;
            let b = hex_value(slice[2])?;
            let [r, g, b] = rgb12_to_rgb24(r, g, b);
            [r, g, b, 255]
        }
        4 if slice.len() == 4 => {
            let r = hex_value(slice[0])?;
            let g = hex_value(slice[1])?;
            let b = hex_value(slice[2])?;
            let a = hex_value(slice[3])?;
            let [r, g, b] = rgb12_to_rgb24(r, g, b);
            [r, g, b, a << 4 | a]
        }
        6 if slice.len() == 6 => {
            [
                parse_hex_nibble_pair(slice[0], slice[1])?,
                parse_hex_nibble_pair(slice[2], slice[3])?,
                parse_hex_nibble_pair(slice[4], slice[5])?,
                255,
            ]
        }
        8 if slice.len() == 8 => {
            [
                parse_hex_nibble_pair(slice[0], slice[1])?,
                parse_hex_nibble_pair(slice[2], slice[3])?,
                parse_hex_nibble_pair(slice[4], slice[5])?,
                parse_hex_nibble_pair(slice[6], slice[7])?,
            ]
        }
        _ => return None,
    };
    Some((rgba, j))
}

fn colors_from_line(line: &[u8]) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < line.len() {
        if line[i] == b'#' {
            if let Some((rgba, next)) = parse_hash_token(line, i) {
                out.push(rgba);
                i = next;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Parse Lospec-style hex text: lines of `#RRGGBB`, optional `#RRGGBBAA`, `#RGB`, comments, or multiple tokens per line.
pub fn parse_hex_palette_text(text: &str) -> Result<Vec<[u8; 4]>, String> {
    let mut colors = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(';') {
            continue;
        }
        let bytes = line.as_bytes();
        for rgba in colors_from_line(bytes) {
            if colors.len() >= MAX_COLORS_PER_PALETTE {
                return Err(format!(
                    "Too many colors (max {MAX_COLORS_PER_PALETTE})"
                ));
            }
            colors.push(rgba);
        }
    }
    if colors.is_empty() {
        Err("No hex colors found (expected lines like #RRGGBB)".to_string())
    } else {
        Ok(colors)
    }
}

/// Export as Lospec-friendly hex: one `#RRGGBB` per line. Uses opaque alpha only; otherwise `#RRGGBBAA`.
pub fn format_hex_palette(colors: &[[u8; 4]]) -> String {
    let mut s = String::new();
    for c in colors {
        if c[3] == 255 {
            s.push_str(&format!("#{:02x}{:02x}{:02x}\n", c[0], c[1], c[2]));
        } else {
            s.push_str(&format!(
                "#{:02x}{:02x}{:02x}{:02x}\n",
                c[0], c[1], c[2], c[3]
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lospec_pico8_style() {
        let text = "#000000\n#1D2B53\n#7E2553\n";
        let v = parse_hex_palette_text(text).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], [0, 0, 0, 255]);
        assert_eq!(v[1], [0x1d, 0x2b, 0x53, 255]);
    }

    #[test]
    fn rgb_short_form() {
        let v = parse_hex_palette_text("#f0a\n").unwrap();
        assert_eq!(v[0], [0xff, 0x00, 0xaa, 255]);
    }

    #[test]
    fn with_alpha() {
        let v = parse_hex_palette_text("#10203040\n").unwrap();
        assert_eq!(v[0], [0x10, 0x20, 0x30, 0x40]);
    }
}
