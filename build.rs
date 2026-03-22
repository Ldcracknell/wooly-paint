use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let target = env::var("TARGET").expect("TARGET");
    let profile = env::var("PROFILE").expect("PROFILE");
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("target"));

    let icon_png = manifest_dir.join("src/assets/icon.png");
    let icon_ico = manifest_dir.join("src/assets/icon.ico");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", icon_png.display());
    println!("cargo:rerun-if-changed={}", icon_ico.display());

    if target.contains("windows") {
        #[cfg(windows)]
        if icon_ico.is_file() {
            let mut res = winres::WindowsResource::new();
            res.set_icon(icon_ico.to_str().expect("icon path is valid UTF-8"));
            res.compile().expect("winres: embed icon");
        }
        #[cfg(not(windows))]
        println!(
            "cargo:warning=Windows target on a non-Windows host: .exe will not embed an icon; build on Windows or use a Windows resource toolchain."
        );
    } else if icon_png.is_file() {
        write_desktop_launcher(
            &manifest_dir,
            &target_dir,
            profile.as_str(),
            target.as_str(),
            &icon_png,
        );
    }
}

fn write_desktop_launcher(
    manifest_dir: &PathBuf,
    target_dir: &PathBuf,
    profile: &str,
    target: &str,
    icon_png: &PathBuf,
) {
    let exe_name = if target.contains("windows") {
        "wooly-paint.exe"
    } else {
        "wooly-paint"
    };
    let exe_path = target_dir.join(profile).join(exe_name);
    let workdir = manifest_dir.to_string_lossy();
    let exe = exe_path.to_string_lossy();
    let icon = icon_png.to_string_lossy();

    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name=Wooly Paint\n\
         Comment=Raster paint\n\
         Exec={exe}\n\
         Icon={icon}\n\
         Path={workdir}\n\
         Terminal=false\n\
         Categories=Graphics;2DGraphics;\n\
         StartupWMClass=dev.woolymelon.WoolyPaint\n"
    );

    let out = target_dir.join(profile).join("wooly-paint.desktop");
    fs::write(&out, desktop).unwrap_or_else(|e| panic!("write {}: {e}", out.display()));
}
