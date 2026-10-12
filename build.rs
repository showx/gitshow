#[path = "src/icon_art.rs"]
mod icon_art;

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let sizes = [16u32, 24, 32, 48, 64, 256];
    let images: Vec<(u32, Vec<u8>)> = sizes.iter().map(|size| (*size, icon_art::render(*size))).collect();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico_path = out_dir.join("gitshow.ico");
    write_ico(&ico_path, &images).expect("写入图标失败");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().unwrap());
        res.set("FileDescription", "GitShow");
        res.set("ProductName", "GitShow");
        res.set("FileVersion", "0.1.0.0");
        res.set("ProductVersion", "0.1.0.0");
        res.set("OriginalFilename", "GitShow.exe");
        res.compile().expect("嵌入程序图标失败");
    }
}

fn write_ico(path: &std::path::Path, images: &[(u32, Vec<u8>)]) -> std::io::Result<()> {
    let mut pngs = Vec::with_capacity(images.len());
    for (size, rgba) in images {
        let mut data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut data, *size, *size);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .map_err(io_error)?
                .write_image_data(rgba)
                .map_err(io_error)?;
        }
        pngs.push((*size, data));
    }

    let mut file = File::create(path)?;
    let count = pngs.len() as u16;
    file.write_all(&0u16.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&count.to_le_bytes())?;

    let mut offset = 6 + pngs.len() as u32 * 16;
    for (size, data) in &pngs {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        file.write_all(&[dim, dim, 0, 0])?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(&(data.len() as u32).to_le_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        offset += data.len() as u32;
    }
    for (_, data) in &pngs {
        file.write_all(data)?;
    }
    Ok(())
}

fn io_error(err: png::EncodingError) -> std::io::Error {
    std::io::Error::other(err.to_string())
}
