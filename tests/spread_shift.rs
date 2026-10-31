//! #16: 見開きの組を 1 頁ずらす（S）。表紙が 1 枚目でない冊のため。冊ごとに覚える。

mod common;

use std::collections::HashMap;

use common::{TmpDir, write_zip};
use eframe::App;
use eframe::egui;
use manga_viewer::view::{Layout, Nav, ViewApp, open_path, plan_nav, prefetch_plan, spread_pages};

#[test]
fn shifted_pairs_start_at_the_first_page() {
    let l = Layout::Shifted;
    assert_eq!(spread_pages(0, l, 5), (0, 1));
    assert_eq!(spread_pages(1, l, 5), (0, 1));
    assert_eq!(spread_pages(2, l, 5), (2, 3));
    assert_eq!(spread_pages(4, l, 5), (4, 4)); // 相手の無い最終頁
    assert_eq!(spread_pages(0, l, 1), (0, 0));
    // 送り: 0-1 → 2-3 → 4、戻りは組の先頭へ
    assert_eq!(plan_nav(0, 1, l, 5), Nav::Page(2));
    assert_eq!(plan_nav(2, 1, l, 5), Nav::Page(4));
    assert_eq!(plan_nav(3, -1, l, 5), Nav::Page(0));
    assert_eq!(
        plan_nav(1, -1, l, 5),
        Nav::Prompt(manga_viewer::neighbor::NavDir::Prev)
    );
    assert_eq!(prefetch_plan(2, l, 20), vec![4, 5, 6, 7, 1, 0]);
    // 単ページ・表紙 1 枚の見開きは従来どおり
    assert_eq!(Layout::from(false), Layout::Single);
    assert_eq!(Layout::from(true), Layout::Spread);
    assert_eq!(spread_pages(1, Layout::Spread, 5), (1, 2));
}

struct Harness {
    ctx: egui::Context,
    app: ViewApp,
}

impl Harness {
    fn open(path: &std::path::Path) -> Harness {
        let (title, book) = open_path(path).unwrap();
        Harness {
            ctx: egui::Context::default(),
            app: ViewApp::new(title, book, 0, None),
        }
    }

    fn key(&mut self, k: egui::Key) -> String {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            events: vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        out.textures_delta.clear();
        // もう 1 フレーム回して、送り後の組を表示の文字列に反映させる
        let mut out = self
            .ctx
            .run_ui(egui::RawInput::default(), |ui| app.show(ui));
        out.textures_delta.clear();
        self.app.status().to_string()
    }
}

fn books(dir: &TmpDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let pages = ["1.png", "2.png", "3.png", "4.png", "5.png"];
    let (a, b) = (dir.0.join("a.zip"), dir.0.join("b.zip"));
    write_zip(&a, &pages);
    write_zip(&b, &pages);
    (a, b)
}

#[test]
fn s_shifts_pairs_and_shows_it() {
    let dir = TmpDir::new("shift-key");
    let (a, _) = books(&dir);
    let mut h = Harness::open(&a);
    let s = h.key(egui::Key::F);
    assert!(s.starts_with("1/5  [見開き（表紙は 1 枚"), "{s}");
    let s = h.key(egui::Key::S);
    assert_eq!(s, "1-2/5  [見開き・組ずらし（S で戻す）]");
    assert!(h.app.shifted());
    assert_eq!(
        h.key(egui::Key::ArrowRight),
        "3-4/5  [見開き・組ずらし（S で戻す）]"
    );
    let s = h.key(egui::Key::ArrowRight);
    assert!(s.starts_with("5/5  [見開き・組ずらし（最後のページ"), "{s}");
    // 戻すと 5 頁目は 4-5 の組
    let s = h.key(egui::Key::S);
    assert!(s.starts_with("4-5/5  [見開き（F で単ページ）"), "{s}");
    assert!(!h.app.shifted());
}

#[derive(Default)]
struct MemStorage(HashMap<String, String>);

impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

#[test]
fn shift_is_remembered_per_book() {
    let dir = TmpDir::new("shift-per-book");
    let (a, b) = books(&dir);
    let mut h = Harness::open(&a);
    h.app = std::mem::replace(&mut h.app, ViewApp::empty()).with_persist(true);
    h.key(egui::Key::F);
    h.key(egui::Key::S);
    assert!(h.app.shifted());
    // 別の冊ではずらさない。元の冊に戻ればずらした状態
    h.app.open_dropped(&b);
    assert!(!h.app.shifted());
    h.app.open_dropped(&a);
    assert!(h.app.shifted());

    // 終了時に保存し、次の起動で戻す
    let mut storage = MemStorage::default();
    h.app.save(&mut storage);
    let mut next = Harness::open(&a);
    assert!(!next.app.shifted());
    next.app.restore_state(&storage);
    assert!(next.app.shifted());
    let mut other = Harness::open(&b);
    other.app.restore_state(&storage);
    assert!(!other.app.shifted());
}
