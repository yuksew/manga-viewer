//! #12: 下端に出る頁スライダー（ヘッドレス egui で同じ Context を使い回してフレームを回す）。

mod common;

use common::{TmpDir, write_zip};
use eframe::egui;
use manga_viewer::view::{SLIDER_ZONE, ViewApp, open_path};

const W: f32 = 900.0;
const H: f32 = 800.0;

struct Harness {
    ctx: egui::Context,
    app: ViewApp,
    _dir: TmpDir,
}

/// 1 フレームの結果: 描かれた文字列と、窓移動（`StartDrag`）を送ったか。
struct Frame {
    text: String,
    start_drag: bool,
    /// 描かれた画像（頁・縮小画像）の矩形
    images: Vec<egui::Rect>,
}

impl Harness {
    fn new(tag: &str, pages: usize) -> Harness {
        let dir = TmpDir::new(tag);
        let names: Vec<String> = (1..=pages).map(|i| format!("{i}.png")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let zip = dir.0.join("a.zip");
        write_zip(&zip, &refs);
        let (title, book) = open_path(&zip).unwrap();
        Harness {
            ctx: egui::Context::default(),
            app: ViewApp::new(title, book, 0, None),
            _dir: dir,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Frame {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W, H),
            )),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        out.textures_delta.clear();
        let mut text = Vec::new();
        let mut images = Vec::new();
        for s in &out.shapes {
            match &s.shape {
                egui::epaint::Shape::Text(t) => text.push(t.galley.text().to_string()),
                egui::epaint::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                    images.push(m.calc_bounds());
                }
                _ => {}
            }
        }
        let start_drag = out.viewport_output.values().any(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::StartDrag))
        });
        Frame {
            text: text.join("\n"),
            start_drag,
            images,
        }
    }

    /// 数フレーム回して表示を落ち着かせる（スライダーは前フレームの配置で当たり判定する）。
    fn settle(&mut self, events: Vec<egui::Event>) -> Frame {
        self.frame(events);
        self.frame(vec![]);
        self.frame(vec![])
    }
}

fn moved(x: f32, y: f32) -> egui::Event {
    egui::Event::PointerMoved(egui::pos2(x, y))
}

fn button(x: f32, y: f32, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: egui::pos2(x, y),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// スライダーのつまみが通る高さ（帯の縦中央）。
const BAR_Y: f32 = H - 24.0;

#[test]
fn slider_shows_only_when_pointer_is_near_bottom() {
    let mut h = Harness::new("slider-hover", 10);
    let f = h.settle(vec![moved(W / 2.0, H / 2.0)]);
    assert!(!f.text.contains("/ 10"), "{}", f.text);
    let f = h.settle(vec![moved(W / 2.0, H - SLIDER_ZONE + 4.0)]);
    assert!(f.text.contains(" 1 / 10"), "{}", f.text);
    // 離れたら消える
    let f = h.settle(vec![moved(W / 2.0, H / 2.0)]);
    assert!(!f.text.contains("/ 10"), "{}", f.text);
}

#[test]
fn clicking_left_end_jumps_to_last_page_right_to_left() {
    let mut h = Harness::new("slider-click", 10);
    h.settle(vec![moved(W / 2.0, BAR_Y)]);
    // 右綴じなので左寄りが後ろの頁。クリックはその場で飛ぶ
    let x = 140.0;
    h.frame(vec![moved(x, BAR_Y), button(x, BAR_Y, true)]);
    let f = h.frame(vec![button(x, BAR_Y, false)]);
    assert!(!f.start_drag, "スライダーの上では窓を動かさない");
    h.settle(vec![]);
    assert!(h.app.page() >= 7, "{}", h.app.page());
    // 左端の外までつまんで動かすと最終頁
    h.frame(vec![button(x, BAR_Y, true)]);
    h.frame(vec![moved(0.0, BAR_Y)]);
    h.frame(vec![button(0.0, BAR_Y, false)]);
    h.settle(vec![]);
    assert_eq!(h.app.page(), 9);
    // 右端の外までなら 1 頁目
    h.frame(vec![moved(x, BAR_Y), button(x, BAR_Y, true)]);
    h.frame(vec![moved(W, BAR_Y)]);
    h.frame(vec![button(W, BAR_Y, false)]);
    h.settle(vec![]);
    assert_eq!(h.app.page(), 0);
}

#[test]
fn dragging_jumps_on_release_and_does_not_move_window() {
    let mut h = Harness::new("slider-drag", 10);
    h.settle(vec![moved(W - 20.0, BAR_Y)]);
    let mut start_drag = false;
    h.frame(vec![button(W - 20.0, BAR_Y, true)]);
    for x in [800.0, 600.0, 450.0] {
        let f = h.frame(vec![moved(x, BAR_Y)]);
        start_drag |= f.start_drag;
    }
    // つまんでいる間は頁を変えず、行き先の番号だけ出す
    assert_eq!(h.app.page(), 0);
    let f = h.frame(vec![moved(450.0, BAR_Y)]);
    assert!(!f.text.contains(" 1 / 10"), "{}", f.text);
    // カーソルが下端から外れても、つまんでいる間は出し続ける
    let f = h.frame(vec![moved(450.0, H / 2.0)]);
    assert!(f.text.contains("/ 10"), "{}", f.text);
    h.frame(vec![moved(450.0, BAR_Y), button(450.0, BAR_Y, false)]);
    h.settle(vec![]);
    assert!(!start_drag, "スライダーをつまんでも窓は動かない");
    let page = h.app.page();
    assert!((3..=6).contains(&page), "中ほどへ飛ぶ: {page}");
    // 矢印キーは頁送りのまま（スライダーがフォーカスを持たない）
    h.frame(vec![egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(h.app.page(), page + 1);
}

/// 縮小画像はスライダーの帯の上に、高さ 180 点で出る。
fn preview_in(f: &Frame) -> Option<egui::Rect> {
    f.images
        .iter()
        .copied()
        .find(|r| (r.height() - 180.0).abs() < 1.0 && r.bottom() < H - 48.0)
}

#[test]
fn dragging_shows_preview_of_target_page_and_hides_on_release() {
    // 頁数を多くして、行き先が頁キャッシュ（表示中の前後）に無いようにする
    let mut h = Harness::new("slider-preview", 40);
    h.settle(vec![moved(W - 20.0, BAR_Y)]);
    h.frame(vec![button(W - 20.0, BAR_Y, true)]);
    let x = 200.0; // 左寄り = 後ろの頁
    let mut found = None;
    let t = std::time::Instant::now();
    while found.is_none() && t.elapsed() < std::time::Duration::from_secs(5) {
        let f = h.frame(vec![moved(x, BAR_Y)]);
        found = preview_in(&f);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let rect = found.expect("つまんでいる間に行き先の縮小画像が出ない");
    // つまみの近く（左寄り）に出る
    assert!(rect.center().x < W / 2.0, "{rect:?}");
    assert_eq!(h.app.page(), 0, "つまんでいる間は飛ばない");
    // 離すと飛んで、縮小画像は消える
    h.frame(vec![button(x, BAR_Y, false)]);
    let f = h.settle(vec![moved(W / 2.0, H / 2.0)]);
    assert!(preview_in(&f).is_none(), "{:?}", f.images);
    assert!(h.app.page() > 20, "{}", h.app.page());
}
