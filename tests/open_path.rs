//! p0-10: DB なしで zip / フォルダを直接開く・空の窓のガイドとドロップ。

mod common;

use common::{TmpDir, write_folder, write_zip};
use eframe::egui;
use manga_viewer::view::{DROP_GUIDE, ViewApp, open_path};

fn frame_text(app: &mut ViewApp) -> String {
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.show(ui));
    out.textures_delta.clear();
    out.shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn open_path_reads_zip_cbz_and_folder() {
    let dir = TmpDir::new("open-path");
    write_zip(&dir.0.join("a.zip"), &["1.png", "2.png"]);
    write_zip(&dir.0.join("b.cbz"), &["1.png"]);
    write_folder(&dir.0.join("f"), &["1.png", "2.png", "3.png"]);
    let (t, b) = open_path(&dir.0.join("a.zip")).unwrap();
    assert_eq!((t.as_str(), b.page_count()), ("a", 2));
    assert_eq!(open_path(&dir.0.join("b.cbz")).unwrap().1.page_count(), 1);
    let (t, b) = open_path(&dir.0.join("f")).unwrap();
    assert_eq!((t.as_str(), b.page_count()), ("f", 3));
}

#[test]
fn open_path_rejects_missing_and_non_zip() {
    let dir = TmpDir::new("open-path-bad");
    std::fs::write(dir.0.join("x.txt"), b"x").unwrap();
    assert!(open_path(&dir.0.join("x.txt")).is_err());
    assert!(open_path(&dir.0.join("none.zip")).is_err());
}

#[test]
fn empty_window_shows_drop_guide() {
    let mut app = ViewApp::empty();
    let text = frame_text(&mut app);
    assert!(text.contains(DROP_GUIDE), "{text}");
    assert_eq!(DROP_GUIDE, "ここに zip またはフォルダをドロップ");
}

#[test]
fn dropped_path_opens_book_and_bad_drop_keeps_it() {
    let dir = TmpDir::new("open-path-drop");
    write_zip(&dir.0.join("a.zip"), &["1.png", "2.png"]);
    let mut app = ViewApp::empty();
    app.open_dropped(&dir.0.join("a.zip"));
    let text = frame_text(&mut app);
    assert!(!text.contains(DROP_GUIDE), "{text}");
    assert!(text.contains("1/2"), "{text}");
    app.open_dropped(&dir.0.join("none.zip"));
    let text = frame_text(&mut app);
    assert!(text.contains("1/2"), "{text}");
}
