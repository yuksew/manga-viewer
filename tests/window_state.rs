//! #13: 窓の状態（見開きの希望）を次の起動に持ち越す。窓の大きさ・位置は eframe が持ち越す。

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::{TmpDir, write_zip};
use eframe::App;
use eframe::egui;
use manga_viewer::view::{ViewApp, open_path};

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

fn book(dir: &TmpDir) -> ViewApp {
    let zip = dir.0.join("a.zip");
    write_zip(&zip, &["1.png", "2.png", "3.png"]);
    let (title, book) = open_path(&zip).unwrap();
    ViewApp::new(title, book, 0, None)
}

fn press_f(app: &mut ViewApp) {
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 800.0),
        )),
        events: vec![egui::Event::Key {
            key: egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    out.textures_delta.clear();
}

#[test]
fn spread_is_saved_and_restored() {
    let dir = TmpDir::new("state-spread");
    let mut storage = MemStorage::default();
    let mut app = book(&dir).with_persist(true);
    press_f(&mut app);
    assert!(app.spread_wanted());
    app.save(&mut storage);
    assert_eq!(storage.0.get("spread").map(String::as_str), Some("1"));

    // 次の起動
    let mut next = book(&dir);
    assert!(!next.spread_wanted());
    next.restore_state(&storage);
    assert!(next.spread_wanted());

    // 単頁に戻して終えれば、次は単頁
    let mut next = next.with_persist(true);
    press_f(&mut next);
    next.save(&mut storage);
    let mut third = book(&dir);
    third.restore_state(&storage);
    assert!(!third.spread_wanted());
}

#[test]
fn windows_that_do_not_persist_leave_storage_alone() {
    let dir = TmpDir::new("state-off");
    let mut storage = MemStorage::default();
    let mut app = book(&dir).with_persist(false);
    press_f(&mut app);
    app.save(&mut storage);
    assert!(storage.0.is_empty());
    // egui 内部の状態は保存しない
    assert!(!app.persist_egui_memory());
}

#[test]
fn measurement_and_auto_close_windows_do_not_persist() {
    let dir = TmpDir::new("state-which");
    // 自動で閉じる窓（--exit-after-ms）は持ち越さない
    let zip = dir.0.join("a.zip");
    write_zip(&zip, &["1.png"]);
    let (title, b) = open_path(&zip).unwrap();
    let auto_close = ViewApp::new(title, b, 0, Some(Duration::from_secs(5)));
    assert!(!auto_close.persists());
    // 普段の窓は持ち越す（MV_PERSIST=0 で切っていなければ）
    if std::env::var_os("MV_PERSIST").is_none() {
        assert!(book(&dir).persists());
    }
}
