//! Generate and embed the original Windows icon using the installed SDK.
//! All ICO/RC/RES intermediates live in Cargo OUT_DIR, not the source tree.

#[path = "src/icon.rs"]
mod icon;
use std::{env, fs, path::PathBuf, process::Command};
/// Emit a multi-resolution ICO, compile it into a resource, and link the app binary.
fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // Windows chooses a suitable size for title bars, taskbar buttons, and Explorer views.
    let sizes = [16u32, 32, 48, 64, 128, 256];
    let mut directory = vec![0u8, 0, 1, 0, sizes.len() as u8, 0];
    let mut images = Vec::new();
    for size in sizes {
        let rgba = icon::rgba(size);
        // ICO BMP entries contain a color bitmap plus a DWORD-aligned monochrome mask.
        let mask_stride = size.div_ceil(32) * 4;
        let mut bmp = Vec::new();
        // BITMAPINFOHEADER height includes both the color image and its mask.
        for n in [40, size, size * 2] {
            bmp.extend_from_slice(&n.to_le_bytes());
        }
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        for n in [0, size * size * 4, 0, 0, 0, 0] {
            bmp.extend_from_slice(&n.to_le_bytes());
        }
        // BMP rows run bottom-up and store BGRA rather than the rasterizer's RGBA.
        for y in (0..size).rev() {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                bmp.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        bmp.resize(bmp.len() + (mask_stride * size) as usize, 0);
        // ICO uses a zero dimension byte for 256; offsets include the entire entry table.
        directory.extend_from_slice(&[size as u8, size as u8, 0, 0]);
        directory.extend_from_slice(&1u16.to_le_bytes());
        directory.extend_from_slice(&32u16.to_le_bytes());
        directory.extend_from_slice(&(bmp.len() as u32).to_le_bytes());
        directory
            .extend_from_slice(&(6 + 16 * sizes.len() as u32 + images.len() as u32).to_le_bytes());
        images.extend(bmp);
    }
    directory.extend(images);
    let ico = out.join("gpt-file-browser.ico");
    fs::write(&ico, directory).unwrap();
    // Cargo is the single source of truth for Explorer's product/file versions.
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric_version = format!(
        "{},{},{},0",
        env::var("CARGO_PKG_VERSION_MAJOR").unwrap(),
        env::var("CARGO_PKG_VERSION_MINOR").unwrap(),
        env::var("CARGO_PKG_VERSION_PATCH").unwrap()
    );
    let rc_file = out.join("app-icon.rc");
    fs::write(
        &rc_file,
        format!(
            "1 ICON \"{}\"\n1 VERSIONINFO\nFILEVERSION {numeric_version}\nPRODUCTVERSION {numeric_version}\nFILEOS 0x40004\nFILETYPE 0x1\nBEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904b0\"\n  BEGIN\n   VALUE \"FileDescription\", \"GPT File Browser\\0\"\n   VALUE \"ProductName\", \"GPT File Browser\\0\"\n   VALUE \"InternalName\", \"gpt-file-browser\\0\"\n   VALUE \"OriginalFilename\", \"GPT File Browser.exe\\0\"\n   VALUE \"FileVersion\", \"{version}\\0\"\n   VALUE \"ProductVersion\", \"{version}\\0\"\n  END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n",
            ico.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    // Explicit RC wins; otherwise locate an installed x64 Windows SDK compiler.
    // No executable is downloaded or installed by the build.
    let rc = env::var_os("RC").map(PathBuf::from).unwrap_or_else(|| {
        let kits = PathBuf::from(env::var_os("ProgramFiles(x86)").expect("Windows SDK required"))
            .join("Windows Kits/10/bin");
        let mut versions: Vec<_> = fs::read_dir(kits)
            .expect("Install Windows SDK or set RC to rc.exe")
            .flatten()
            .map(|e| e.path())
            .collect();
        versions.sort();
        versions.reverse();
        versions
            .into_iter()
            .map(|p| p.join("x64/rc.exe"))
            .find(|p| p.is_file())
            .expect("Windows SDK rc.exe not found; set RC")
    });
    let res = out.join("app-icon.res");
    let status = Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc_file)
        .status()
        .expect("Could not run Windows resource compiler");
    assert!(status.success(), "Icon resource compilation failed");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
