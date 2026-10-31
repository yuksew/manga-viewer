//! p0-14: 見開きの実機不具合の回帰（描画された画像の枚数・位置、F の入力経路、状態表示、キャッシュ）。

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{TmpDir, write_zip};
use eframe::egui;
use manga_viewer::view::{
    SPREAD_MIN_WIDTH, ViewApp, keep_range, mode_label, open_path, prefetch_plan,
};

fn key_event(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn app_for(path: &Path) -> ViewApp {
    let (title, book) = open_path(path).unwrap();
    ViewApp::new(title, book, 0, None)
}

/// 幅 `width` の窓で 1 フレーム回し、(描画された全文字列, 画像の矩形を左から順に) を返す。
fn run(app: &mut ViewApp, width: f32, events: Vec<egui::Event>) -> (String, Vec<egui::Rect>) {
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 800.0),
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
    rects.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    (text.join("\n"), rects)
}

fn zip_of(tag: &str, n: usize) -> (TmpDir, std::path::PathBuf) {
    let dir = TmpDir::new(tag);
    let names: Vec<String> = (1..=n).map(|i| format!("{i}.png")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let zip = dir.0.join("a.zip");
    write_zip(&zip, &refs);
    (dir, zip)
}

#[test]
fn spread_draws_two_images_side_by_side_at_real_window_widths() {
    let (_dir, zip) = zip_of("sd-draw", 3);
    // 実機の既定幅 900 でも、閾値ちょうどでも 2 枚（左右の半分ずつ）
    for width in [900.0, SPREAD_MIN_WIDTH] {
        let mut app = app_for(&zip);
        assert_eq!(run(&mut app, width, vec![]).1.len(), 1);
        // 1 枚目は表紙なので見開きでも 1 枚
        let (_, rects) = run(&mut app, width, vec![key_event(egui::Key::F)]);
        assert_eq!(rects.len(), 1, "width {width}: {rects:?}");
        let (_, rects) = run(&mut app, width, vec![key_event(egui::Key::ArrowRight)]);
        assert_eq!(rects.len(), 2, "width {width}: {rects:?}");
        // 2 頁は窓の中央で隙間なく接する
        assert!((rects[0].max.x - width / 2.0).abs() < 0.5, "{rects:?}");
        assert!((rects[1].min.x - width / 2.0).abs() < 0.5, "{rects:?}");
    }
}

/// 頁ごとに一辺の違う正方形 PNG を入れた zip（描画矩形の大きさで頁を見分けるため）。
fn zip_with_sizes(tag: &str, sides: &[u32]) -> (TmpDir, std::path::PathBuf) {
    use std::io::Write;
    let dir = TmpDir::new(tag);
    let zip = dir.0.join("a.zip");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&zip).unwrap());
    for (i, side) in sides.iter().enumerate() {
        let img = image::RgbImage::from_pixel(*side, *side, image::Rgb([200, 100, 50]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        w.start_file(
            format!("{}.png", i + 1),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        w.write_all(&buf.into_inner()).unwrap();
    }
    w.finish().unwrap();
    (dir, zip)
}

#[test]
fn spread_puts_first_page_on_the_right_and_second_on_the_left() {
    // 表紙の次の組: 2 ページ目は 20px、3 ページ目は 60px の正方形。右綴じなら右半分が 20px、左半分が 60px
    let (_dir, zip) = zip_with_sizes("sd-rtl", &[40, 20, 60]);
    let mut app = app_for(&zip);
    run(&mut app, 900.0, vec![]);
    run(&mut app, 900.0, vec![key_event(egui::Key::F)]);
    let (_, rects) = run(&mut app, 900.0, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(rects.len(), 2, "{rects:?}");
    let (left, right) = (rects[0], rects[1]);
    // 大きさの違う頁でも中央で接する（間に隙間を作らない）
    assert!(
        (left.max.x - 450.0).abs() < 0.5 && (right.min.x - 450.0).abs() < 0.5,
        "{rects:?}"
    );
    assert!(
        (right.width() - 20.0).abs() < 0.5,
        "右は 1 ページ目: {rects:?}"
    );
    assert!(
        (left.width() - 60.0).abs() < 0.5,
        "左は 2 ページ目: {rects:?}"
    );
}

#[test]
fn f_works_from_text_event_only_and_not_twice() {
    let (_dir, zip) = zip_of("sd-text", 3);
    let mut app = app_for(&zip);
    // IME 経由などでキーイベントが無くテキストだけ来た場合
    let (text, _) = run(&mut app, 1000.0, vec![egui::Event::Text("f".into())]);
    assert!(text.contains("1/3  [見開き（表紙は 1 枚）]"), "{text}");
    // キーとテキストが同じフレームに来ても 1 回だけ切り替わる
    let both = vec![key_event(egui::Key::F), egui::Event::Text("f".into())];
    let (text, _) = run(&mut app, 1000.0, both);
    assert!(
        text.contains("1/3  [") && text.contains("単頁（F で見開き）"),
        "{text}"
    );
}

#[test]
fn narrow_window_says_why_it_is_single() {
    let (_dir, zip) = zip_of("sd-narrow", 3);
    let mut app = app_for(&zip);
    let (text, rects) = run(&mut app, 500.0, vec![key_event(egui::Key::F)]);
    assert!(text.contains("見開きにできません"), "{text}");
    assert!(text.contains("500"), "{text}");
    assert_eq!(rects.len(), 1);
}

#[test]
fn toggling_f_reuses_cached_pages() {
    let (_dir, zip) = zip_of("sd-cache", 4);
    let mut app = app_for(&zip);
    run(&mut app, 1000.0, vec![]);
    run(&mut app, 1000.0, vec![key_event(egui::Key::F)]);
    // 先読み（裏スレッド）が全頁を埋めるまで待つ
    let t = Instant::now();
    while app.cached_pages() < 4 && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
        run(&mut app, 1000.0, vec![]);
    }
    assert_eq!(app.cached_pages(), 4);
    let before = app.sync_loads();
    run(&mut app, 1000.0, vec![key_event(egui::Key::ArrowRight)]);
    run(&mut app, 1000.0, vec![key_event(egui::Key::F)]);
    run(&mut app, 1000.0, vec![key_event(egui::Key::F)]);
    assert_eq!(app.sync_loads(), before, "先読み済みの頁は同期復号しない");
}

#[test]
fn mode_label_and_prefetch_plan() {
    assert!(mode_label(false, 900.0, false, 0, 5).contains("単頁"));
    assert!(mode_label(true, 900.0, true, 0, 5).contains("表紙"));
    assert!(mode_label(true, 900.0, true, 1, 5).contains("F で単頁"));
    assert!(mode_label(true, 900.0, true, 4, 5).contains("F で単頁")); // 4 は 3-4 の組
    assert!(mode_label(true, 600.0, false, 0, 5).contains("見開きにできません"));
    assert!(mode_label(true, 900.0, true, 5, 6).contains("最終頁"));
    assert_eq!(prefetch_plan(5, false, 20), vec![6, 7, 4, 3]);
    assert_eq!(prefetch_plan(3, true, 20), vec![5, 6, 7, 8, 2, 1, 0]);
    assert_eq!(prefetch_plan(4, true, 20), prefetch_plan(3, true, 20)); // 4 は 3-4 の組
    assert_eq!(prefetch_plan(0, true, 20), vec![1, 2, 3, 4]);
    assert_eq!(prefetch_plan(0, false, 2), vec![1]);
    assert_eq!(keep_range(10, 12), 4..12);
    assert_eq!(keep_range(0, 3), 0..3);
}
