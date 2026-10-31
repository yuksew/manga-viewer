//! 窓のドラッグ移動を、冊を開いていないとき（空の窓・一覧）と閲覧中で確かめる。
//! 押して閾値（6 点）を超えて動かすと `StartDrag` を送る。

mod common;

use std::fs;

use common::{TmpDir, write_zip};
use eframe::egui;
use manga_viewer::index::Index;
use manga_viewer::library::Library;
use manga_viewer::neighbor::NavDir;
use manga_viewer::view::{ViewApp, open_path};

const W: f32 = 900.0;
const H: f32 = 800.0;

struct Harness {
    ctx: egui::Context,
    app: ViewApp,
    time: f64,
}

impl Harness {
    fn new(app: ViewApp) -> Harness {
        Harness {
            ctx: egui::Context::default(),
            app,
            time: 0.0,
        }
    }

    /// 1 フレーム回し、(描かれた文字列と位置, StartDrag を送ったか) を返す。
    fn frame(&mut self, events: Vec<egui::Event>) -> (Vec<(String, egui::Pos2)>, bool) {
        self.time += 0.05;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W, H),
            )),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        out.textures_delta.clear();
        let texts = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos)),
                _ => None,
            })
            .collect();
        let start = out.viewport_output.values().any(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::StartDrag))
        });
        (texts, start)
    }

    /// `at` で押して右へ 20 点動かして離す。途中で StartDrag を送ったか。
    fn drag_from(&mut self, at: egui::Pos2) -> bool {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![]);
        let mut started = self.frame(vec![button(at, true)]).1;
        for dx in [4.0, 10.0, 20.0] {
            started |= self
                .frame(vec![egui::Event::PointerMoved(at + egui::vec2(dx, 0.0))])
                .1;
        }
        self.frame(vec![button(at + egui::vec2(20.0, 0.0), false)]);
        started
    }

    fn click(&mut self, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }
}

#[test]
fn empty_window_drags_from_anywhere() {
    for at in [egui::pos2(W / 2.0, H / 2.0), egui::pos2(10.0, 10.0)] {
        let mut h = Harness::new(ViewApp::empty());
        assert!(h.drag_from(at), "{at:?}");
    }
}

fn library(t: &TmpDir) -> ViewApp {
    let root = t.0.join("root");
    for name in ["v1", "v2", "v3"] {
        write_zip(&root.join(format!("{name}/{name}.zip")), &["001.png"]);
    }
    let db = t.0.join("db/index.db");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    Index::open(&db).unwrap().reindex(&root).unwrap();
    ViewApp::with_library(Library::open(&db, Some(&root)).unwrap())
}

#[test]
fn library_drags_from_covers_and_header_and_still_selects_by_click() {
    let t = TmpDir::new("drag-library");
    let mut h = Harness::new(library(&t));
    h.frame(vec![]);
    // 2 つ目の表紙のマス（1 マス 200×300、見出しの下）の上から
    let second = egui::pos2(300.0, 150.0);
    assert!(h.drag_from(second), "表紙の上から窓を動かせない");
    assert_eq!(
        h.app.library().unwrap().selected(),
        0,
        "ドラッグでは選ばない"
    );
    // 見出し（ルートと冊数）の上からも
    assert!(
        h.drag_from(egui::pos2(30.0, 8.0)),
        "見出しの上から窓を動かせない"
    );
    // 表紙の無い余白からも
    assert!(
        h.drag_from(egui::pos2(W - 30.0, H - 30.0)),
        "余白から窓を動かせない"
    );
    // 動かさずに押して離せば、従来どおりその冊を選ぶ
    h.click(second);
    assert_eq!(h.app.library().unwrap().selected(), 1);
    assert!(h.app.in_library());
}

#[test]
fn reading_view_still_does_not_drag_from_prompt_buttons() {
    let dir = TmpDir::new("drag-prompt");
    write_zip(&dir.0.join("v1.zip"), &["1.png"]);
    write_zip(&dir.0.join("v2.zip"), &["1.png"]);
    let (title, book) = open_path(&dir.0.join("v1.zip")).unwrap();
    let mut h = Harness::new(ViewApp::new(title, book, 0, None));
    // 頁の上からは動かせる
    assert!(h.drag_from(egui::pos2(W / 2.0, H / 2.0)));
    h.frame(vec![egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(h.app.prompt(), Some(NavDir::Next));
    let (texts, _) = h.frame(vec![]);
    let pos = texts
        .iter()
        .find(|(t, _)| t == "いいえ")
        .expect("いいえ が描かれていない")
        .1
        + egui::vec2(8.0, 8.0);
    assert!(!h.drag_from(pos), "ボタンの上からは動かさない");
}
