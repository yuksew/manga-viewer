#![allow(dead_code)]
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use zip::write::SimpleFileOptions;

static SEQ: AtomicU32 = AtomicU32::new(0);

/// `tests/.tmp/<一意名>/` を作り、Drop で消す。
pub struct TmpDir(pub PathBuf);

impl TmpDir {
    pub fn new(tag: &str) -> TmpDir {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/.tmp")
            .join(format!("{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn png_bytes() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 100, 50]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// 指定名のエントリを持つ合成 zip を書く。
pub fn write_zip(path: &Path, names: &[&str]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(File::create(path).unwrap());
    let png = png_bytes();
    for n in names {
        w.start_file(*n, SimpleFileOptions::default()).unwrap();
        w.write_all(&png).unwrap();
    }
    w.finish().unwrap();
}

pub fn write_folder(dir: &Path, names: &[&str]) {
    fs::create_dir_all(dir).unwrap();
    let png = png_bytes();
    for n in names {
        fs::write(dir.join(n), &png).unwrap();
    }
}

/// 名前と中身を指定した合成 zip を書く。
pub fn write_zip_data(path: &Path, entries: &[(&str, &[u8])]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(File::create(path).unwrap());
    for (n, data) in entries {
        w.start_file(*n, SimpleFileOptions::default()).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap();
}

/// 最小の妥当な JPEG（SOF0 1x1 + SOS + EOI）。
pub fn jpeg_bytes() -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend([0xFF, 0xC0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0]);
    v.extend([0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 0x3F, 0]);
    v.extend([0x12, 0x34, 0xFF, 0xD9]);
    v
}

/// 最小の妥当な GIF（1x1、トレーラ 3B 終わり）。
pub fn gif_bytes() -> Vec<u8> {
    let mut v = b"GIF89a".to_vec();
    v.extend([1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    v.extend([0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3B]);
    v
}

/// RIFF サイズが実長と合う WebP 形のバイト列（VP8L チャンク、中身はダミー）。
pub fn webp_bytes() -> Vec<u8> {
    let payload = [0x2F, 0, 0, 0, 0, 0, 0, 0];
    let mut v = b"RIFF".to_vec();
    v.extend((4 + 8 + payload.len() as u32).to_le_bytes());
    v.extend(b"WEBPVP8L");
    v.extend((payload.len() as u32).to_le_bytes());
    v.extend(payload);
    v
}

/// 実際に符号化した 3x2 画像（`image` クレート）。復号まで通る本物のバイト列。
fn encode_real(format: image::ImageFormat) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(3, 2, image::Rgb([10, 120, 240]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, format)
        .unwrap();
    buf.into_inner()
}

pub fn real_jpeg_bytes() -> Vec<u8> {
    encode_real(image::ImageFormat::Jpeg)
}

pub fn real_gif_bytes() -> Vec<u8> {
    encode_real(image::ImageFormat::Gif)
}

pub fn real_webp_bytes() -> Vec<u8> {
    encode_real(image::ImageFormat::WebP)
}

/// ディレクトリの一覧読みを Everyone に拒否する（読めない子ディレクトリの再現、#5）。
/// Drop で拒否を外す（外さないと `TmpDir` が消せない）。
pub struct DenyList(pub PathBuf);

impl DenyList {
    pub fn new(dir: &Path) -> DenyList {
        let ok = std::process::Command::new("icacls")
            .arg(dir)
            .args(["/deny", "*S-1-1-0:(RD)"])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "icacls /deny に失敗");
        DenyList(dir.to_path_buf())
    }
}

impl Drop for DenyList {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(&self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

/// エントリ名を生のバイトのまま書いた無圧縮 zip（UTF-8 フラグなし）。
/// zip クレートの書き手は名前を UTF-8 で書くので、古い CP932 の cbz を再現するのに使う（#6）。
pub fn write_zip_raw_names(path: &Path, entries: &[(&[u8], &[u8])]) {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let (n, len) = (name.len() as u16, data.len() as u32);
        // ローカルヘッダ: 署名, version, flags=0, method=0, time, date, crc, sizes, name len, extra len
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        for v in [20u16, 0, 0, 0, 0x21] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, len, len] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        // 中央ディレクトリ
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        for v in [20u16, 20, 0, 0, 0, 0x21] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, len, len] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [n, 0, 0, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    let count = entries.len() as u16;
    for v in [0u16, 0, count, count] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, out).unwrap();
}
