//! p0-6b: 復号（バイト → RGBA）と view の頁読み込み。窓は開かない。

mod common;

use std::io::Cursor;

use common::{TmpDir, write_zip_data};
use image::{DynamicImage, ImageFormat, RgbaImage};
use manga_viewer::book::Book;
use manga_viewer::decode::{DecodeError, MAX_PIXELS, decode_rgba};
use manga_viewer::imgcheck::Kind;
use manga_viewer::scan::BookKind;
use manga_viewer::view::{load_page, step};

/// 左上だけ赤・他は青の 3x2 を、指定形式で符号化する。
fn encode(format: ImageFormat) -> Vec<u8> {
    let mut img = RgbaImage::from_pixel(3, 2, image::Rgba([0, 0, 255, 255]));
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let img = match format {
        // JPEG はアルファを持てない
        ImageFormat::Jpeg => DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(img).to_rgb8()),
        _ => DynamicImage::ImageRgba8(img),
    };
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, format).unwrap();
    buf.into_inner()
}

fn check_3x2(bytes: &[u8], kind: Kind, lossless: bool) {
    let d = decode_rgba(bytes).unwrap();
    assert_eq!(d.kind, kind);
    assert_eq!((d.width, d.height), (3, 2));
    assert_eq!(d.rgba.len(), 3 * 2 * 4);
    let px = |i: usize| &d.rgba[i * 4..i * 4 + 4];
    if lossless {
        assert_eq!(px(0), [255, 0, 0, 255]);
        assert_eq!(px(5), [0, 0, 255, 255]);
    } else {
        // JPEG は非可逆: 赤が優勢 / 青が優勢であることだけ見る
        assert!(px(0)[0] > 150 && px(0)[2] < 100, "{:?}", px(0));
        assert!(px(5)[2] > 150 && px(5)[0] < 100, "{:?}", px(5));
    }
}

#[test]
fn decodes_png() {
    check_3x2(&encode(ImageFormat::Png), Kind::Png, true);
}

#[test]
fn decodes_jpeg() {
    check_3x2(&encode(ImageFormat::Jpeg), Kind::Jpeg, false);
}

#[test]
fn decodes_gif() {
    // GIF は 256 色パレットだが 2 色なので厳密一致する
    check_3x2(&encode(ImageFormat::Gif), Kind::Gif, true);
}

#[test]
fn decodes_webp() {
    check_3x2(&encode(ImageFormat::WebP), Kind::Webp, true);
}

#[test]
fn rejects_unknown_signature() {
    assert!(matches!(
        decode_rgba(b"not an image at all"),
        Err(DecodeError::Invalid(_))
    ));
    assert!(matches!(decode_rgba(b""), Err(DecodeError::Invalid(_))));
}

#[test]
fn truncated_images_are_errors_not_panics() {
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
    ] {
        let full = encode(format);
        for cut in [1, full.len() / 2, full.len() - 1] {
            // p0-8a: 読める所まで読めた場合（PNG の IEND 末尾 1 バイト欠けなど）は成功してよいが、
            // 必ず修復印が付く。静かに成功してはならない。
            if let Ok(d) = decode_rgba(&full[..cut]) {
                assert!(d.repair.is_some(), "{format:?} cut={cut}");
            }
        }
    }
}

#[test]
fn header_valid_but_body_garbage_does_not_panic() {
    // imgcheck は通るが中身が壊れている PNG（IDAT を潰す）
    let mut png = encode(ImageFormat::Png);
    let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
    for b in &mut png[idat..idat + 6] {
        *b = 0xAB;
    }
    assert!(decode_rgba(&png).is_err());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

/// IHDR だけ巨大寸法に書き換えた PNG（CRC も合わせる）。画素データは小さいまま。
fn huge_png(width: u32, height: u32) -> Vec<u8> {
    let mut png = encode(ImageFormat::Png);
    png[16..20].copy_from_slice(&width.to_be_bytes());
    png[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    png
}

#[test]
fn huge_dimensions_are_refused_before_decoding() {
    let side = (MAX_PIXELS as f64).sqrt() as u32 + 1;
    match decode_rgba(&huge_png(side, side)) {
        Err(DecodeError::TooLarge { width, height }) => assert_eq!((width, height), (side, side)),
        other => panic!("TooLarge のはず: {other:?}"),
    }
}

#[test]
fn page_load_uses_magic_not_extension() {
    // .jpg の名前だが中身は PNG、.png の名前だが中身は壊れ
    let dir = TmpDir::new("decode-view");
    let zip = dir.0.join("b.zip");
    let png = encode(ImageFormat::Png);
    let broken = png[..png.len() / 2].to_vec();
    write_zip_data(
        &zip,
        &[("1.jpg", &png), ("2.png", &broken), ("3.png", b"zzzz")],
    );
    let book = Book::open(&zip, BookKind::Zip).unwrap();

    let ok = load_page(&book, 0).unwrap();
    assert_eq!((ok.kind, ok.width, ok.height), (Kind::Png, 3, 2));
    assert_eq!(ok.rgba.len(), 24);

    let err = load_page(&book, 1).unwrap_err();
    assert!(!err.is_empty());
    let err = load_page(&book, 2).unwrap_err();
    assert!(err.contains("署名"), "{err}");
    assert!(load_page(&book, 3).is_err());
}

#[test]
fn step_clamps_to_page_range() {
    assert_eq!(step(0, -1, 5), 0);
    assert_eq!(step(0, 1, 5), 1);
    assert_eq!(step(4, 1, 5), 4);
    assert_eq!(step(9, 0, 5), 4);
    assert_eq!(step(0, 1, 0), 0);
}
