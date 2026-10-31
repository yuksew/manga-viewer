//! #14: 全画面の切り替え（F11・頁の上のダブルクリック・全画面中の Esc）。
//! 窓が全画面かどうかは `RawInput` の窓情報で与える（実際の切り替えは送ったコマンドで見る）。

mod common;

use common::{TmpDir, write_zip};
use eframe::egui;
use manga_viewer::view::{ViewApp, open_path};

const W: f32 = 900.0;
const H: f32 = 800.0;

struct Harness {
    ctx: egui::Context,
    app: ViewApp,
    time: f64,
    /// 窓が全画面か（次のフレームの窓情報に載せる）
    full: bool,
    _dir: TmpDir,
}

/// 1 フレームの結果: 描かれた文字列と、送った全画面・閉じるのコマンド。
struct Frame {
    text: String,
    fullscreen: Option<bool>,
    close: bool,
}

impl Harness {
    fn new(tag: &str) -> Harness {
        let dir = TmpDir::new(tag);
        let zip = dir.0.join("題名.zip");
        write_zip(&zip, &["1.png", "2.png", "3.png"]);
        let (title, book) = open_path(&zip).unwrap();
        Harness {
            ctx: egui::Context::default(),
            app: ViewApp::new(title, book, 0, None),
            time: 0.0,
            full: false,
            _dir: dir,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Frame {
        self.time += 0.05;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W, H),
            )),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .fullscreen = Some(self.full);
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        out.textures_delta.clear();
        let mut text = Vec::new();
        for s in &out.shapes {
            if let egui::epaint::Shape::Text(t) = &s.shape {
                text.push(t.galley.text().to_string());
            }
        }
        let commands: Vec<_> = out
            .viewport_output
            .values()
            .flat_map(|v| v.commands.clone())
            .collect();
        let fullscreen = commands.iter().find_map(|c| match c {
            egui::ViewportCommand::Fullscreen(on) => Some(*on),
            _ => None,
        });
        if let Some(on) = fullscreen {
            self.full = on;
        }
        Frame {
            text: text.join("\n"),
            fullscreen,
            close: commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close)),
        }
    }

    /// (x, y) でダブルクリックし、最後のフレームの結果を返す。
    fn double_click(&mut self, x: f32, y: f32) -> Frame {
        let pos = egui::pos2(x, y);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(pos), button(true)]);
        self.frame(vec![button(false)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)])
    }
}

fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn f11_toggles_fullscreen() {
    let mut h = Harness::new("fs-f11");
    h.frame(vec![]);
    assert_eq!(h.frame(vec![key(egui::Key::F11)]).fullscreen, Some(true));
    assert_eq!(h.frame(vec![key(egui::Key::F11)]).fullscreen, Some(false));
}

#[test]
fn double_click_on_page_toggles_fullscreen() {
    let mut h = Harness::new("fs-double");
    h.frame(vec![]);
    let f = h.double_click(W / 2.0, H / 2.0);
    assert_eq!(f.fullscreen, Some(true));
    // 頁は送らない
    assert_eq!(h.app.page(), 0);
    // 続けてすぐ叩くと 3 回目以降のクリックになるので、間をあけてもう一度
    h.time += 1.0;
    let f = h.double_click(W / 2.0, H / 2.0);
    assert_eq!(f.fullscreen, Some(false));
}

#[test]
fn single_click_does_nothing() {
    let mut h = Harness::new("fs-single");
    h.frame(vec![]);
    let pos = egui::pos2(W / 2.0, H / 2.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let a = h.frame(vec![egui::Event::PointerMoved(pos), button(true)]);
    let b = h.frame(vec![button(false)]);
    assert_eq!((a.fullscreen, b.fullscreen), (None, None));
}

#[test]
fn esc_in_fullscreen_only_leaves_fullscreen() {
    let mut h = Harness::new("fs-esc");
    h.frame(vec![]);
    h.frame(vec![key(egui::Key::F11)]);
    let f = h.frame(vec![key(egui::Key::Escape)]);
    assert_eq!(f.fullscreen, Some(false));
    assert!(!f.close, "全画面中の Esc で窓を閉じない");
    // 窓に戻ってからの Esc は従来どおり閉じる
    let f = h.frame(vec![key(egui::Key::Escape)]);
    assert!(f.close);
}

#[test]
fn double_click_on_slider_does_not_toggle() {
    let mut h = Harness::new("fs-slider");
    let bar = egui::pos2(W / 2.0, H - 24.0);
    // 下端にカーソルを置いてスライダーを出す
    h.frame(vec![egui::Event::PointerMoved(bar)]);
    h.frame(vec![]);
    let f = h.double_click(bar.x, bar.y);
    assert_eq!(f.fullscreen, None);
}

#[test]
fn fullscreen_label_shows_title() {
    let mut h = Harness::new("fs-title");
    let f = h.frame(vec![]);
    assert!(!f.text.contains("題名"), "{}", f.text);
    h.frame(vec![key(egui::Key::F11)]);
    let f = h.frame(vec![]);
    assert!(f.text.contains("題名  1/3"), "{}", f.text);
}
