//! #1: 冊の一覧画面を、ヘッドレス egui でフレームを回して確かめる（同じ Context を使い回す）。

mod common;

use std::fs;
use std::path::Path;

use common::{TmpDir, write_zip, write_zip_data};
use eframe::egui;
use manga_viewer::index::Index;
use manga_viewer::library::Library;
use manga_viewer::view::ViewApp;

struct Harness {
    ctx: egui::Context,
    app: ViewApp,
}

impl Harness {
    fn new(db: &Path, root: &Path) -> Harness {
        let library = Library::open(db, Some(root)).unwrap();
        Harness {
            ctx: egui::Context::default(),
            app: ViewApp::with_library(library),
        }
    }

    /// 1 フレーム回して描かれた文字列と、新しく上がったテクスチャの寸法を返す。
    fn frame(&mut self, events: Vec<egui::Event>) -> (String, Vec<[usize; 2]>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 1000.0),
            )),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        let sizes = out
            .textures_delta
            .set
            .values()
            .flat_map(|d| d.iter())
            .map(|d| d.image.size())
            .collect();
        let mut text = String::new();
        for clipped in &out.shapes {
            collect_text(&clipped.shape, &mut text);
        }
        out.textures_delta.clear();
        (text, sizes)
    }

    fn key(&mut self, key: egui::Key) -> String {
        self.frame(vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }])
        .0
    }
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

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 200, 30]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// root に v1, v2, v10（自然順）と壊れた冊を置き、索引する。
fn setup(t: &TmpDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = t.0.join("root");
    for name in ["v10", "v2", "v1"] {
        write_zip(
            &root.join(format!("{name}/{name}.zip")),
            &["001.png", "002.png"],
        );
    }
    fs::create_dir_all(root.join("x")).unwrap();
    fs::write(root.join("x/zbroken.zip"), b"not a zip").unwrap();
    let db = t.0.join("db/index.db");
    Index::open(&db).unwrap().reindex(&root).unwrap();
    (db, root)
}

#[test]
fn lists_books_in_natural_order_with_broken_marked() {
    let t = TmpDir::new("lib-list");
    let (db, root) = setup(&t);
    let mut h = Harness::new(&db, &root);
    let (text, _) = h.frame(vec![]);
    assert!(h.app.in_library());
    let pos = |s: &str| {
        text.find(s)
            .unwrap_or_else(|| panic!("{s} が無い:\n{text}"))
    };
    assert!(pos("v1\n") < pos("v2\n") && pos("v2\n") < pos("v10\n"));
    assert!(text.contains("4 冊"), "{text}");
    assert!(text.contains("2 頁"));
    assert!(text.contains("壊れた冊") && text.contains("×"));
}

#[test]
fn enter_opens_selected_book_and_esc_returns_to_list() {
    let t = TmpDir::new("lib-open");
    let (db, root) = setup(&t);
    let mut h = Harness::new(&db, &root);
    h.frame(vec![]);
    h.key(egui::Key::ArrowRight); // v1 → v2
    assert_eq!(h.app.library().unwrap().selected(), 1);
    h.key(egui::Key::Enter);
    assert!(!h.app.in_library());
    assert_eq!(h.app.title(), "v2");
    let text = h.frame(vec![]).0;
    assert!(text.contains("v2  1/2"), "{text}");

    // 閲覧中の Esc は窓を閉じずに一覧へ戻り、読んでいた冊が選ばれている
    h.key(egui::Key::Escape);
    assert!(h.app.in_library());
    assert_eq!(h.app.library().unwrap().selected(), 1);
    let text = h.frame(vec![]).0;
    assert!(text.contains("4 冊"), "{text}");
}

#[test]
fn broken_book_shows_reason_instead_of_opening() {
    let t = TmpDir::new("lib-broken");
    let (db, root) = setup(&t);
    let mut h = Harness::new(&db, &root);
    h.frame(vec![]);
    h.key(egui::Key::End); // 自然順の最後 = zbroken
    h.key(egui::Key::Enter);
    assert!(h.app.in_library());
    let text = h.frame(vec![]).0;
    assert!(text.contains("開けません（zbroken）"), "{text}");
}

#[test]
fn covers_from_db_are_shown_as_textures() {
    let t = TmpDir::new("lib-covers");
    let root = t.0.join("root");
    write_zip_data(&root.join("a/a.zip"), &[("001.png", &png(700, 1000))]);
    let db = t.0.join("db/index.db");
    let mut index = Index::open(&db).unwrap();
    index.reindex(&root).unwrap();
    index.generate_covers(None, None).unwrap();
    drop(index);

    let mut h = Harness::new(&db, &root);
    let (_, sizes) = h.frame(vec![]);
    assert!(
        sizes.contains(&[238, 340]),
        "表紙のテクスチャが無い: {sizes:?}"
    );
    assert_eq!(h.app.library().unwrap().cover_textures(), 1);
    // 2 フレーム目は読み直さない
    let (_, sizes) = h.frame(vec![]);
    assert!(!sizes.contains(&[238, 340]));
}
