//! #15: 上部の頁番号・モードは上端にカーソルを当てたときだけ頁の上に重ねて出す。通知は隠さない。

mod common;

use common::{TmpDir, write_zip};
use eframe::egui;
use manga_viewer::view::{TOP_ZONE, ViewApp, open_path};

const W: f32 = 900.0;
const H: f32 = 800.0;

/// 1 フレーム回し、(描かれた文字列, 頁画像の矩形) を返す。`pointer` があればそこへカーソルを置く。
fn frame(
    ctx: &egui::Context,
    app: &mut ViewApp,
    pointer: Option<egui::Pos2>,
    events: Vec<egui::Event>,
) -> (String, Vec<egui::Rect>) {
    let mut events = events;
    if let Some(p) = pointer {
        events.insert(0, egui::Event::PointerMoved(p));
    }
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(W, H),
        )),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    out.textures_delta.clear();
    let mut text = Vec::new();
    let mut rects = Vec::new();
    for s in &out.shapes {
        match &s.shape {
            egui::epaint::Shape::Text(t) => text.push(t.galley.text().to_string()),
            egui::epaint::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                rects.push(m.calc_bounds());
            }
            _ => {}
        }
    }
    (text.join("\n"), rects)
}

fn app(dir: &TmpDir, pages: &[&str]) -> ViewApp {
    let zip = dir.0.join("a.zip");
    write_zip(&zip, pages);
    let (title, book) = open_path(&zip).unwrap();
    ViewApp::new(title, book, 0, None)
}

#[test]
fn status_is_drawn_only_near_top() {
    let dir = TmpDir::new("top-hover");
    let mut app = app(&dir, &["1.png", "2.png", "3.png"]);
    let ctx = egui::Context::default();
    let (text, _) = frame(&ctx, &mut app, Some(egui::pos2(W / 2.0, H / 2.0)), vec![]);
    assert!(!text.contains("1/3"), "{text}");
    assert_eq!(app.status(), "1/3  [単頁（F で見開き）]");
    let (text, _) = frame(
        &ctx,
        &mut app,
        Some(egui::pos2(W / 2.0, TOP_ZONE - 4.0)),
        vec![],
    );
    assert!(text.contains("1/3  [単頁"), "{text}");
    // 離れたら消える
    let (text, _) = frame(&ctx, &mut app, Some(egui::pos2(W / 2.0, H / 2.0)), vec![]);
    assert!(!text.contains("1/3"), "{text}");
}

#[test]
fn notice_is_shown_without_hover() {
    let dir = TmpDir::new("top-notice");
    let mut app = app(&dir, &["1.png"]);
    let ctx = egui::Context::default();
    let key = |k| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let middle = Some(egui::pos2(W / 2.0, H / 2.0));
    frame(&ctx, &mut app, middle, vec![key(egui::Key::ArrowRight)]);
    let (text, _) = frame(&ctx, &mut app, middle, vec![key(egui::Key::Enter)]);
    // 次の冊の候補が無い通知は、カーソルが真ん中でも出る（頁番号は出さない）
    assert!(text.contains("次の冊はありません"), "{text}");
    assert!(!text.contains("1/1"), "{text}");
}

#[test]
fn page_uses_the_whole_window_height() {
    let dir = TmpDir::new("top-full");
    let mut app = app(&dir, &["1.png"]);
    let ctx = egui::Context::default();
    // 1×1 の頁は拡大しないので、縦の中央に来る（上のラベル分ずれない）
    let (_, rects) = frame(&ctx, &mut app, None, vec![]);
    assert_eq!(rects.len(), 1, "{rects:?}");
    assert!((rects[0].center().y - H / 2.0).abs() < 1.0, "{rects:?}");
}
