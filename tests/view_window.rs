//! p0-7a: 窓に出るバイトとエラー文言を、ヘッドレス egui で 1 フレーム回して確かめる。

mod common;

use std::io::Cursor;

use common::{TmpDir, write_zip_data};
use eframe::egui;
use image::{DynamicImage, ImageFormat, RgbaImage};
use manga_viewer::book::Book;
use manga_viewer::decode::decode_rgba;
use manga_viewer::scan::BookKind;
use manga_viewer::view::ViewApp;

/// 左上だけ赤・他は青の不透明 3x2 PNG。
fn png_3x2() -> Vec<u8> {
    let mut img = RgbaImage::from_pixel(3, 2, image::Rgba([0, 0, 255, 255]));
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img)
        .write_to(&mut buf, ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

fn open_zip(tag: &str, entries: &[(&str, &[u8])]) -> (TmpDir, Book) {
    let dir = TmpDir::new(tag);
    let zip = dir.0.join("b.zip");
    write_zip_data(&zip, entries);
    let book = Book::open(&zip, BookKind::Zip).unwrap();
    (dir, book)
}

/// 1 フレーム回して（テクスチャ差分, 描画された全文字列）を返す。
fn frame(app: &mut ViewApp, key: Option<egui::Key>) -> (Vec<egui::ColorImage>, String) {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    if let Some(key) = key {
        raw.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    }
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    let images = out
        .textures_delta
        .set
        .values()
        .flat_map(|deltas| deltas.iter())
        .map(|d| match &d.image {
            egui::ImageData::Color(c) => (**c).clone(),
        })
        .collect();
    let mut text = String::new();
    for clipped in &out.shapes {
        collect_text(&clipped.shape, &mut text);
    }
    out.textures_delta.clear(); // 描画しないので未適用の差分を捨てる
    // 上部の頁番号・モードはカーソルを当てたときだけ描くので、表示の文字列も足す（#15）
    text.push_str(app.status());
    (images, text)
}

fn collect_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

/// 3x2 のテクスチャ（フォントアトラスを除く）。
fn page_texture(images: &[egui::ColorImage]) -> Option<&egui::ColorImage> {
    images.iter().find(|c| c.size == [3, 2])
}

#[test]
fn window_texture_bytes_match_zip_image() {
    let png = png_3x2();
    let (_dir, book) = open_zip("view-tex", &[("1.png", &png), ("2.png", &png)]);
    let expected = decode_rgba(&png).unwrap();

    let mut app = ViewApp::new("題名".into(), book, 0, None);
    let (images, text) = frame(&mut app, None);
    let tex = page_texture(&images).expect("頁テクスチャが渡っていない");
    let got: Vec<u8> = tex.pixels.iter().flat_map(|p| p.to_array()).collect();
    assert_eq!(got, expected.rgba);
    assert_eq!(&got[0..4], [255, 0, 0, 255]); // 左上の赤が残っている
    assert!(text.contains("1/2  ["), "{text}");
    assert!(!text.contains("表示できません"), "{text}");
}

#[test]
fn broken_page_shows_error_text_then_recovers_on_next_page() {
    let png = png_3x2();
    let (_dir, book) = open_zip("view-err", &[("1.png", b"zzzz"), ("2.png", &png)]);

    let mut app = ViewApp::new("t".into(), book, 0, None);
    let (images, text) = frame(&mut app, None);
    assert!(page_texture(&images).is_none());
    assert!(text.contains("この頁は表示できません"), "{text}");
    assert!(text.contains("署名"), "{text}");

    let (images, text) = frame(&mut app, Some(egui::Key::ArrowRight));
    assert!(page_texture(&images).is_some());
    assert!(!text.contains("表示できません"), "{text}");
    assert!(text.contains("2/2"), "{text}");
}

#[test]
fn body_corrupt_page_shows_error_text() {
    // imgcheck の署名検査は通るが本体が壊れている PNG
    let mut png = png_3x2();
    let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
    for b in &mut png[idat..idat + 6] {
        *b = 0xAB;
    }
    let (_dir, book) = open_zip("view-corrupt", &[("1.png", &png)]);
    let mut app = ViewApp::new("t".into(), book, 0, None);
    let (images, text) = frame(&mut app, None);
    assert!(page_texture(&images).is_none());
    assert!(text.contains("この頁は表示できません"), "{text}");
}

#[test]
fn out_of_range_start_page_clamps_to_last() {
    let png = png_3x2();
    let (_dir, book) = open_zip(
        "view-clamp",
        &[("1.png", &png), ("2.png", &png), ("3.png", &png)],
    );
    let mut app = ViewApp::new("t".into(), book, 999, None);
    let (_, text) = frame(&mut app, None);
    assert!(text.contains("3/3"), "{text}");
}
