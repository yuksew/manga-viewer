//! #9: JPEG 頁の「復号 + 窓へ縮小 + RGBA 化」の内訳を測る。
//!
//! DCT スケーリング（jpeg-decoder で 1/2 などに縮めながら復号）も試したが、漫画らしい頁では
//! 1 割程度しか速くならず（時間の大半はハフマン復号で縮めても減らない）、採らなかった。
//!
//! `cargo test --release --test perf_decode -- --ignored --nocapture --test-threads=1`
//!
//! 合否で落とさない（数値を持ち帰るのが目的）。

use std::time::Instant;

use fast_image_resize::{self as fir, ResizeOptions, Resizer};
use image::{DynamicImage, GrayImage, ImageEncoder, RgbImage};
use manga_viewer::decode::{decode_fit, fit_within};

/// 漫画らしい頁（白地に枠線・網点・少しのノイズ）。`color` なら薄く色を載せる。
fn manga(w: u32, h: u32, color: bool) -> DynamicImage {
    let mut s = 0x9E37_79B9u32;
    let gray = GrayImage::from_fn(w, h, |x, y| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let frame = x % (w / 3) < 6 || y % (h / 3) < 6;
        let tone = (x / 4 + y / 4) % 4 == 0 && (y / (h / 6)) % 2 == 1;
        let v: u8 = if frame || tone { 30 } else { 245 };
        image::Luma([v.saturating_sub((s & 15) as u8)])
    });
    if !color {
        return DynamicImage::ImageLuma8(gray);
    }
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        let v = gray.get_pixel(x, y).0[0];
        let tint = ((x * 255 / w) as u8, (y * 255 / h) as u8);
        image::Rgb([v.min(255 - tint.0 / 4), v, v.min(255 - tint.1 / 4)])
    }))
}

fn jpeg(img: &DynamicImage) -> Vec<u8> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85)
        .write_image(
            img.as_bytes(),
            img.width(),
            img.height(),
            img.color().into(),
        )
        .unwrap();
    buf
}

fn median_ms(mut f: impl FnMut()) -> f64 {
    let mut v: Vec<f64> = (0..15)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// 今の経路の内訳: 検査（imgcheck）・zune-jpeg の全面復号・縮小 + RGBA 化。
#[test]
#[ignore = "release で手動実行するベンチ"]
fn jpeg_fit_breakdown() {
    let (tw, th) = (1350u32, 1500u32);
    println!("BENCH source\tcolor\tcheck ms\tdecode ms\tfit total ms");
    for color in [false, true] {
        for (w, h) in [(1500u32, 2200u32), (3500, 5000)] {
            let bytes = jpeg(&manga(w, h, color));
            let check = median_ms(|| {
                manga_viewer::imgcheck::check(&bytes).unwrap();
            });
            let decode = median_ms(|| {
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap();
            });
            let total = median_ms(|| {
                decode_fit(&bytes, tw, th).unwrap();
            });
            println!(
                "BENCH {w}x{h}\t{}\t{check:.2}\t{decode:.1}\t{total:.1}",
                if color { "rgb" } else { "gray" }
            );
        }
    }
}

/// 縮小 + RGBA 化の内訳（今の経路と同じ呼び方を外から再現する）。
#[test]
#[ignore = "release で手動実行するベンチ"]
fn jpeg_fit_after_decode() {
    let (tw, th) = (1350u32, 1500u32);
    println!("BENCH source\tcolor\tlimits decode ms\tresize ms\trgba ms");
    for color in [false, true] {
        for (w, h) in [(1500u32, 2200u32), (3500, 5000)] {
            let bytes = jpeg(&manga(w, h, color));
            let read = || {
                let mut r = image::ImageReader::with_format(
                    std::io::Cursor::new(&bytes),
                    image::ImageFormat::Jpeg,
                );
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(512 << 20);
                r.limits(limits);
                r.decode().unwrap()
            };
            let decode = median_ms(|| {
                read();
            });
            let img = read();
            let (fw, fh) = fit_within((w, h), (tw, th));
            let resize = median_ms(|| {
                let mut dst = DynamicImage::new(fw, fh, img.color());
                let options = ResizeOptions::new()
                    .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Bilinear));
                Resizer::new().resize(&img, &mut dst, &options).unwrap();
            });
            let mut dst = DynamicImage::new(fw, fh, img.color());
            Resizer::new()
                .resize(&img, &mut dst, &ResizeOptions::new())
                .unwrap();
            let rgba = median_ms(|| {
                let _ = dst.clone().into_rgba8();
            });
            println!(
                "BENCH {w}x{h}\t{}\t{decode:.1}\t{resize:.1}\t{rgba:.1}",
                if color { "rgb" } else { "gray" }
            );
        }
    }
}

/// RGBA 化: image の汎用変換と `decode::into_rgba` の比較（結果が同じことも確かめる）。
#[test]
#[ignore = "release で手動実行するベンチ"]
fn rgba_conversion() {
    println!("BENCH kind\timage ms\tinto_rgba ms");
    for color in [false, true] {
        let img = manga(1023, 1500, color);
        assert_eq!(
            img.clone().into_rgba8().into_raw(),
            manga_viewer::decode::into_rgba(img.clone())
        );
        let generic = median_ms(|| {
            let _ = img.clone().into_rgba8();
        });
        let fast = median_ms(|| {
            let _ = manga_viewer::decode::into_rgba(img.clone());
        });
        println!(
            "BENCH {}\t{generic:.2}\t{fast:.2}",
            if color { "rgb" } else { "gray" }
        );
    }
}
