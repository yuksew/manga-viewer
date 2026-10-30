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
