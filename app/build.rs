//! Embeds the product icon from `assets/branding/icon.png` in the Windows executables of every
//! package that names this file as its build script.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Included relative to this file, so each package using the script finds it; a change rebuilds
/// and therefore reruns the script.
const ICON: &[u8] = include_bytes!("../assets/branding/icon.png");
/// Icon sizes Windows picks from for Explorer views, the taskbar and title bars.
const ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() {
    println!("cargo:rerun-if-env-changed=RC");
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") != "windows" || target("CARGO_CFG_TARGET_ENV") != "msvc" {
        return;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let ico = out.join("icon.ico");
    std::fs::write(&ico, icon_file(ICON)).expect("write icon.ico");
    let script = out.join("icon.rc");
    let ico_path = ico.display().to_string().replace('\\', "\\\\");
    std::fs::write(&script, format!("1 ICON \"{ico_path}\"\n")).expect("write icon.rc");
    let Some(compiler) = resource_compiler() else {
        println!("cargo:warning=rc.exe not found; the executable is built without an icon");
        return;
    };
    let res = out.join("icon.res");
    let compiled = Command::new(&compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&script)
        .status()
        .is_ok_and(|status| status.success());
    if compiled {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!("cargo:warning={} failed; the executable is built without an icon", compiler.display());
    }
}

/// An ICO file holding the PNG `source` resized to each of [`ICON_SIZES`] as PNG frames.
fn icon_file(source: &[u8]) -> Vec<u8> {
    let image = image::load_from_memory(source)
        .expect("decode icon.png")
        .into_rgba8();
    let frames: Vec<(u32, Vec<u8>)> = ICON_SIZES
        .iter()
        .map(|&size| {
            let resized =
                image::imageops::resize(&image, size, size, image::imageops::FilterType::Lanczos3);
            let mut png = std::io::Cursor::new(Vec::new());
            resized
                .write_to(&mut png, image::ImageFormat::Png)
                .expect("encode icon frame");
            (size, png.into_inner())
        })
        .collect();
    let count = u16::try_from(frames.len()).expect("few icon frames");
    let mut file = Vec::new();
    file.extend_from_slice(&0u16.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(count);
    for (size, png) in &frames {
        // A dimension of 256 is stored as 0.
        let dimension = u8::try_from(*size).unwrap_or(0);
        let bytes = u32::try_from(png.len()).expect("icon frame fits u32");
        file.extend_from_slice(&[dimension, dimension, 0, 0]);
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&bytes.to_le_bytes());
        file.extend_from_slice(&offset.to_le_bytes());
        offset += bytes;
    }
    for (_, png) in frames {
        file.extend_from_slice(&png);
    }
    file
}

/// `RC` when set, else the newest x64 `rc.exe` in the Windows 10/11 SDK.
fn resource_compiler() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("RC") {
        return Some(PathBuf::from(rc));
    }
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join(r"x64\rc.exe").is_file())
        .collect();
    versions.sort();
    versions.pop().map(|version| version.join(r"x64\rc.exe"))
}
