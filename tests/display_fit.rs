//! #8: 窓サイズへの縮小（decode_fit）・不透明頁の無変換アップロード・開いた瞬間の表紙仮表示。

mod common;

use std::time::Duration;

use common::{TmpDir, write_zip_data};
use eframe::egui;
use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};
use manga_viewer::decode::{Repair, decode_fit, decode_rgba, fit_within};
use manga_viewer::index::Index;
use manga_viewer::library::Library;
use manga_viewer::scan::BookKind;
use manga_viewer::view::ViewApp;

fn encode(img: DynamicImage, fmt: ImageFormat) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, fmt).unwrap();
    buf.into_inner()
}

/// 縦長の RGB PNG（左半分が赤、右半分が青）。
fn rgb_png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, _| {
        if x < w / 2 {
            image::Rgb([220, 10, 10])
        } else {
            image::Rgb([10, 10, 220])
        }
    });
    encode(DynamicImage::ImageRgb8(img), ImageFormat::Png)
}

#[test]
fn fit_within_keeps_aspect_and_never_enlarges() {
    assert_eq!(fit_within((1500, 2200), (900, 1000)), (682, 1000));
    assert_eq!(fit_within((3000, 2000), (900, 1000)), (900, 600));
    assert_eq!(fit_within((300, 400), (900, 1000)), (300, 400));
    assert_eq!(fit_within((10, 100_000), (900, 1000)), (1, 1000));
    assert_eq!(fit_within((100, 100), (0, 0)), (1, 1));
}

#[test]
fn decode_fit_shrinks_and_reports_source_size() {
    let d = decode_fit(&rgb_png(1500, 2200), 900, 1000).unwrap();
    assert_eq!((d.width, d.height), (682, 1000));
    assert_eq!(d.source_size, (1500, 2200));
    assert_eq!(d.rgba.len(), 682 * 1000 * 4);
    assert!(d.opaque);
    // 縮めても色の配置は保たれる（左は赤、右は青）
    let px = |x: usize, y: usize| &d.rgba[(y * 682 + x) * 4..(y * 682 + x) * 4 + 4];
    assert!(px(10, 500)[0] > 200 && px(10, 500)[2] < 40);
    assert!(px(670, 500)[2] > 200 && px(670, 500)[0] < 40);
}

#[test]
fn decode_fit_without_shrink_matches_decode_rgba() {
    let png = rgb_png(300, 400);
    let a = decode_fit(&png, 900, 1000).unwrap();
    let b = decode_rgba(&png).unwrap();
    assert_eq!((a.width, a.height, a.source_size), (300, 400, (300, 400)));
    assert_eq!(a.rgba, b.rgba);
}

#[test]
fn alpha_images_are_not_marked_opaque() {
    let img = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 128]));
    let d = decode_fit(
        &encode(DynamicImage::ImageRgba8(img), ImageFormat::Png),
        2,
        2,
    )
    .unwrap();
    assert!(!d.opaque);
    assert_eq!((d.width, d.height), (2, 2));
    // グレー画像は不透明
    let gray = image::GrayImage::from_pixel(8, 8, image::Luma([100]));
    let d = decode_fit(
        &encode(DynamicImage::ImageLuma8(gray), ImageFormat::Png),
        4,
        4,
    )
    .unwrap();
    assert!(d.opaque);
    assert_eq!(&d.rgba[..4], &[100, 100, 100, 255]);
}

#[test]
fn repair_mark_survives_shrinking() {
    let good = encode(
        DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, image::Rgb([9, 9, 9]))),
        ImageFormat::Jpeg,
    );
    let mut bad = good.clone();
    bad.extend_from_slice(b"junk");
    let d = decode_fit(&bad, 16, 16).unwrap();
    assert_eq!(d.repair, Some(Repair::TrailingGarbage { len: good.len() }));
    assert_eq!((d.width, d.height), (16, 16));
}

fn frame(
    ctx: &egui::Context,
    app: &mut ViewApp,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> Vec<egui::ColorImage> {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    let images = out
        .textures_delta
        .set
        .values()
        .flat_map(|d| d.iter())
        .map(|d| match &d.image {
            egui::ImageData::Color(c) => (**c).clone(),
        })
        .collect();
    out.textures_delta.clear();
    images
}

/// 窓に収まる大きさのテクスチャになり、不透明頁の画素は変換なしでも正しい色。窓を広げると作り直す。
#[test]
fn page_texture_fits_window_and_grows_with_it() {
    let t = TmpDir::new("fit-view");
    let zip = t.0.join("b.zip");
    write_zip_data(&zip, &[("1.png", &rgb_png(1500, 2200))]);
    let book = manga_viewer::book::Book::open(&zip, BookKind::Zip).unwrap();
    let mut app = ViewApp::new("b".into(), book, 0, None);
    let ctx = egui::Context::default();

    let images = frame(&ctx, &mut app, egui::vec2(900.0, 1000.0), vec![]);
    let page = images
        .iter()
        .find(|c| c.size[1] > 100 && c.size[0] != 2048)
        .expect("頁のテクスチャ");
    assert!(
        page.size[0] <= 900 && page.size[1] <= 1000,
        "{:?}",
        page.size
    );
    assert_eq!(
        page.pixels[page.size[0] * 100 + 5],
        egui::Color32::from_rgb(220, 10, 10)
    );

    // 狭めても作り直さない
    let images = frame(&ctx, &mut app, egui::vec2(600.0, 700.0), vec![]);
    assert!(!images.iter().any(|c| c.size[1] > 100 && c.size[0] != 2048));
    // 広げると大きく作り直す。ただしテクスチャの最大辺（ヘッドレスは 2048）は超えない
    let images = frame(&ctx, &mut app, egui::vec2(1600.0, 2400.0), vec![]);
    let page = images
        .iter()
        .find(|c| c.size[1] > 100 && c.size[0] != 2048)
        .expect("作り直したテクスチャ");
    assert_eq!(page.size, [1396, 2048]);
}

/// 一覧から開いた直後は先頭ページを同期で復号せず、表紙を出しておいて裏で読み、届いたら差し替える。
#[test]
fn opening_from_library_shows_cover_until_first_page_arrives() {
    let t = TmpDir::new("fit-placeholder");
    let root = t.0.join("root");
    write_zip_data(
        &root.join("a/a.zip"),
        &[
            ("1.png", &rgb_png(1500, 2200)),
            ("2.png", &rgb_png(1500, 2200)),
        ],
    );
    let db = t.0.join("db/index.db");
    let mut index = Index::open(&db).unwrap();
    index.reindex(&root).unwrap();
    index.generate_covers(None, None).unwrap();
    drop(index);

    let mut app = ViewApp::with_library(Library::open(&db, Some(&root)).unwrap());
    let ctx = egui::Context::default();
    let size = egui::vec2(900.0, 1000.0);
    frame(&ctx, &mut app, size, vec![]); // 一覧（表紙テクスチャを読む）
    let enter = egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&ctx, &mut app, size, vec![enter]);
    assert!(!app.in_library());
    frame(&ctx, &mut app, size, vec![]); // 開いた最初の閲覧フレーム
    assert_eq!(app.sync_loads(), 0, "先頭ページを同期で復号した");
    assert!(app.showing_placeholder());

    let mut arrived = false;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(20));
        frame(&ctx, &mut app, size, vec![]);
        if !app.showing_placeholder() {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "先頭頁が届かない");
    assert_eq!(app.sync_loads(), 0);
    assert!(app.cached_pages() >= 1);
}
