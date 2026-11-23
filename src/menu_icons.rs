//! Lucide menu icons rasterized in `build.rs` (same approach as `tool_cursors::tool_dropdown_icon_texture`).

use gtk::gdk;
use gtk::glib;
use std::cell::RefCell;
use std::collections::HashMap;

pub fn menu_bar_icon_texture(alias: &str, ui_dark: bool) -> gdk::Texture {
    thread_local! {
        static CACHE: RefCell<HashMap<(&'static str, bool), gdk::Texture>> = RefCell::new(HashMap::new());
    }
    let (key, png) = menu_icon_png(alias, ui_dark);
    CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some(tex) = cache.get(&(key, ui_dark)) {
            return tex.clone();
        }
        let bytes = glib::Bytes::from_static(png);
        let tex = gdk::Texture::from_bytes(&bytes).expect("menu icon png");
        cache.insert((key, ui_dark), tex.clone());
        tex
    })
}

fn menu_icon_png(alias: &str, ui_dark: bool) -> (&'static str, &'static [u8]) {
    macro_rules! icons {
        ($($name:literal),* $(,)?) => {
            match (alias, ui_dark) {
                $(
                    ($name, true) => ($name, include_bytes!(concat!(env!("OUT_DIR"), "/menu/", $name, "_dark.png"))),
                    ($name, false) => ($name, include_bytes!(concat!(env!("OUT_DIR"), "/menu/", $name, "_light.png"))),
                )*
                _ => panic!("unknown menu icon alias: {alias}"),
            }
        };
    }
    icons!(
        "file",
        "new",
        "open",
        "recent",
        "save",
        "save_as",
        "canvas",
        "resize",
        "flip_x",
        "flip_y",
        "rotate",
        "grid",
        "settings",
        "keybinds",
        "updates",
        "theme",
        "theme_default",
        "theme_light",
        "theme_dark",
        "palettes",
        "import_hex",
        "export_hex",
        "manage_palettes",
        "image",
    )
}
