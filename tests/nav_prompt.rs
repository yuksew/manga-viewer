//! p0-12: ホイール送り・見開きトグル・最終/最初頁での次冊/前冊プロンプトと候補解決。

mod common;

use std::path::Path;

use common::{TmpDir, write_folder, write_zip};
use eframe::egui;
use manga_viewer::index::{BookRow, Index, Status};
use manga_viewer::neighbor::{NavDir, neighbor_in_dir, neighbor_in_rows, resolve_neighbor};
use manga_viewer::scan::BookKind;
use manga_viewer::view::{
    Nav, PromptLayout, SPREAD_MIN_WIDTH, ViewApp, open_path, plan_nav, spread_active,
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

fn wheel(y: f32) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, y),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: egui::Modifiers::NONE,
    }
}

/// 幅 `width` の窓で 1 フレーム回し、描画された全文字列を返す。
fn frame_w(app: &mut ViewApp, width: f32, events: Vec<egui::Event>) -> String {
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
    let mut text: Vec<String> = out
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect();
    // 上部の頁番号・モードはカーソルを当てたときだけ描くので、表示の文字列も足す（#15）
    text.push(app.status().to_string());
    text.join("\n")
}

fn frame(app: &mut ViewApp, events: Vec<egui::Event>) -> String {
    frame_w(app, 1000.0, events)
}

fn app_for(path: &Path) -> ViewApp {
    let (title, book) = open_path(path).unwrap();
    ViewApp::new(title, book, 0, None)
}

fn names(p: Option<std::path::PathBuf>) -> Option<String> {
    p.map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
}

fn row(path: &Path, status: Status) -> BookRow {
    BookRow {
        id: 0,
        path: path.to_path_buf(),
        kind: BookKind::Zip,
        title: String::new(),
        page_count: 1,
        status,
        error: None,
        mtime_ns: 0,
        size: 0,
        scanned_at: 0,
    }
}

#[test]
fn plan_nav_single_and_spread_boundaries() {
    assert_eq!(plan_nav(0, 1, false, 3), Nav::Page(1));
    assert_eq!(plan_nav(2, 1, false, 3), Nav::Prompt(NavDir::Next));
    assert_eq!(plan_nav(0, -1, false, 3), Nav::Prompt(NavDir::Prev));
    // 見開き: 表紙（索引 0）は 1 枚、以降は索引 1-2, 3-4… の組で送る。最終判定は組の末尾
    assert_eq!(plan_nav(0, 1, true, 6), Nav::Page(1));
    assert_eq!(plan_nav(1, 1, true, 6), Nav::Page(3));
    assert_eq!(plan_nav(3, 1, true, 6), Nav::Page(5)); // 相手の無い最終頁へ
    assert_eq!(plan_nav(5, 1, true, 6), Nav::Prompt(NavDir::Next));
    assert_eq!(plan_nav(3, 1, true, 5), Nav::Prompt(NavDir::Next)); // 3-4 の 4=last
    assert_eq!(plan_nav(4, 1, true, 5), Nav::Prompt(NavDir::Next)); // 4 は 3-4 の組
    assert_eq!(plan_nav(3, -1, true, 6), Nav::Page(1));
    assert_eq!(plan_nav(2, -1, true, 6), Nav::Page(0)); // 2 は 1-2 の組 → 表紙
    assert_eq!(plan_nav(1, -1, true, 6), Nav::Page(0));
    assert_eq!(plan_nav(0, -1, true, 6), Nav::Prompt(NavDir::Prev));
    // ホイールをまとめて回した分は組単位で進み、端で止まる（端を越える確認は出さない）
    assert_eq!(plan_nav(0, 3, true, 6), Nav::Page(5));
    assert_eq!(plan_nav(1, 5, true, 6), Nav::Page(5));
    assert_eq!(plan_nav(5, -9, true, 6), Nav::Page(0));
    // 1 頁だけの冊は両端
    assert_eq!(plan_nav(0, 1, true, 1), Nav::Prompt(NavDir::Next));
}

#[test]
fn narrow_window_forces_single_page() {
    assert!(spread_active(true, SPREAD_MIN_WIDTH));
    assert!(!spread_active(true, SPREAD_MIN_WIDTH - 1.0));
    assert!(!spread_active(false, 5000.0));
}

#[test]
fn neighbor_in_dir_uses_natural_order_and_skips_non_books() {
    let dir = TmpDir::new("nb-dir");
    write_zip(&dir.0.join("a2.zip"), &["1.png"]);
    write_zip(&dir.0.join("a10.zip"), &["1.png"]);
    write_zip(&dir.0.join("a1.zip"), &["1.png"]);
    write_folder(&dir.0.join("b"), &["1.png"]);
    std::fs::create_dir_all(dir.0.join("a5-empty")).unwrap();
    std::fs::write(dir.0.join("a3.txt"), b"x").unwrap();

    let n = |f: &str, d| names(neighbor_in_dir(&dir.0.join(f), d));
    assert_eq!(n("a1.zip", NavDir::Next).as_deref(), Some("a2.zip"));
    assert_eq!(n("a2.zip", NavDir::Next).as_deref(), Some("a10.zip"));
    assert_eq!(n("a10.zip", NavDir::Next).as_deref(), Some("b"));
    assert_eq!(n("b", NavDir::Next), None);
    assert_eq!(n("b", NavDir::Prev).as_deref(), Some("a10.zip"));
    assert_eq!(n("a1.zip", NavDir::Prev), None);
}

#[test]
fn neighbor_in_rows_skips_broken_and_falls_back_to_dir() {
    let dir = TmpDir::new("nb-rows");
    for f in ["x1.zip", "x2.zip", "x3.zip", "x4.zip"] {
        write_zip(&dir.0.join(f), &["1.png"]);
    }
    let p = |f: &str| dir.0.join(f);
    let rows = vec![
        row(&p("x1.zip"), Status::Ok),
        row(&p("x2.zip"), Status::Broken),
        row(&p("x3.zip"), Status::Ok),
    ];
    let n = |f: &str, d| names(resolve_neighbor(&p(f), d, Some(&rows)));
    assert_eq!(n("x1.zip", NavDir::Next).as_deref(), Some("x3.zip"));
    assert_eq!(n("x3.zip", NavDir::Prev).as_deref(), Some("x1.zip"));
    assert_eq!(n("x3.zip", NavDir::Next), None); // 索引末尾（x4 は索引外）
    assert_eq!(neighbor_in_rows(&rows, &p("x1.zip"), NavDir::Prev), None);
    // 索引に載っていない冊はフォルダ順
    assert_eq!(n("x4.zip", NavDir::Prev).as_deref(), Some("x3.zip"));
    assert_eq!(
        names(resolve_neighbor(&p("x2.zip"), NavDir::Next, None)).as_deref(),
        Some("x3.zip")
    );
}

#[test]
fn wheel_pages_one_per_event_and_keys_do_not_double_count() {
    let dir = TmpDir::new("nav-wheel");
    write_zip(&dir.0.join("a.zip"), &["1.png", "2.png", "3.png", "4.png"]);
    let mut app = app_for(&dir.0.join("a.zip"));
    frame(&mut app, vec![]);
    frame(&mut app, vec![wheel(-1.0)]);
    assert_eq!(app.page(), 1);
    frame(&mut app, vec![wheel(-1.0), wheel(-1.0)]);
    assert_eq!(app.page(), 3);
    frame(&mut app, vec![wheel(1.0)]);
    assert_eq!(app.page(), 2);
    // キーとホイールが同じフレームに来ても +1 だけ
    frame(&mut app, vec![key_event(egui::Key::ArrowLeft), wheel(1.0)]);
    assert_eq!(app.page(), 1);
}

#[test]
fn spread_toggle_shows_two_pages_and_steps_by_two() {
    let dir = TmpDir::new("nav-spread");
    write_zip(
        &dir.0.join("a.zip"),
        &["1.png", "2.png", "3.png", "4.png", "5.png"],
    );
    let mut app = app_for(&dir.0.join("a.zip"));
    assert!(frame(&mut app, vec![]).contains("1/5  ["));
    // 1 枚目は表紙として 1 枚で出す
    let text = frame(&mut app, vec![key_event(egui::Key::F)]);
    assert!(
        text.contains("1/5  [見開き（表紙は 1 枚・S で組をずらす）]"),
        "{text}"
    );
    let text = frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert!(text.contains("2-3/5  ["), "{text}");
    assert_eq!(app.page(), 1);
    let text = frame(&mut app, vec![wheel(-1.0)]);
    assert!(text.contains("4-5/5  ["), "{text}");
    assert!(app.prompt().is_none());
    frame(&mut app, vec![wheel(-1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    // 単頁に戻すと組の先頭の頁
    app.answer_prompt(false);
    let text = frame(&mut app, vec![key_event(egui::Key::F)]);
    assert!(text.contains("4/5  ["), "{text}");
    // 単頁で組の後ろ側（5 頁目）にいて見開きにすると、その組（4-5）に揃う
    frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    let text = frame(&mut app, vec![key_event(egui::Key::F)]);
    assert!(text.contains("4-5/5  ["), "{text}");
    assert_eq!(app.page(), 3);
}

#[test]
fn narrow_window_stays_single_even_when_toggled() {
    let dir = TmpDir::new("nav-narrow");
    write_zip(&dir.0.join("a.zip"), &["1.png", "2.png", "3.png"]);
    let mut app = app_for(&dir.0.join("a.zip"));
    let text = frame_w(&mut app, 400.0, vec![key_event(egui::Key::F)]);
    assert!(text.contains("1/3  ["), "{text}");
    frame_w(&mut app, 400.0, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.page(), 1);
}

#[test]
fn spread_last_is_right_page() {
    let dir = TmpDir::new("nav-spread-last");
    write_zip(&dir.0.join("a.zip"), &["1.png", "2.png", "3.png", "4.png"]);
    let mut app = app_for(&dir.0.join("a.zip"));
    frame(&mut app, vec![key_event(egui::Key::F)]);
    frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.page(), 1); // 2-3 の見開き
    let text = frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.page(), 3); // 相手の無い 4 頁目は 1 枚
    assert!(text.contains("4/4  [見開き（最終頁は 1 枚）]"), "{text}");
    frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
}

#[test]
fn next_prompt_opens_next_file_without_db() {
    let dir = TmpDir::new("nav-next");
    write_zip(&dir.0.join("v1.zip"), &["1.png", "2.png"]);
    write_zip(&dir.0.join("v2.zip"), &["1.png", "2.png", "3.png"]);
    write_folder(&dir.0.join("v3"), &["1.png"]);
    let mut app = app_for(&dir.0.join("v1.zip"));
    frame(&mut app, vec![key_event(egui::Key::PageDown)]);
    assert_eq!(app.page(), 1);
    let text = frame(&mut app, vec![wheel(-1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    assert!(text.contains("次の冊を開きますか"), "{text}");
    // プロンプト中の送り入力（キー・同じ向きのホイール）は無視
    frame(&mut app, vec![key_event(egui::Key::ArrowLeft)]);
    frame(
        &mut app,
        vec![key_event(egui::Key::ArrowRight), wheel(-1.0)],
    );
    assert_eq!(app.prompt(), Some(NavDir::Next));
    assert_eq!((app.title(), app.page()), ("v1", 1));
    // Enter = はい
    let text = frame(&mut app, vec![key_event(egui::Key::Enter)]);
    assert_eq!((app.title(), app.page()), ("v2", 0), "{text}");
    assert!(app.prompt().is_none());
    // 3 頁目まで送ってフォルダ冊へ
    frame(&mut app, vec![wheel(-1.0)]);
    frame(&mut app, vec![wheel(-1.0)]);
    frame(&mut app, vec![wheel(-1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    frame(&mut app, vec![key_event(egui::Key::Enter)]);
    assert_eq!(app.title(), "v3");
    // 最後の冊の最終頁: はい → 通知を出して現在冊のまま
    frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    let text = frame(&mut app, vec![key_event(egui::Key::Enter)]);
    assert_eq!(app.title(), "v3");
    assert!(text.contains("次の冊はありません"), "{text}");
    // 次の送りで通知は消える
    let text = frame(&mut app, vec![wheel(1.0)]);
    assert!(!text.contains("次の冊はありません"), "{text}");
}

#[test]
fn prev_prompt_no_keeps_book_yes_opens_previous() {
    let dir = TmpDir::new("nav-prev");
    write_zip(&dir.0.join("v1.zip"), &["1.png"]);
    write_zip(&dir.0.join("v2.zip"), &["1.png", "2.png"]);
    let mut app = app_for(&dir.0.join("v2.zip"));
    let text = frame(&mut app, vec![wheel(1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Prev));
    assert!(text.contains("前の冊を開きますか"), "{text}");
    // Esc = いいえ（窓は閉じず、プロンプトだけ消える）
    frame(&mut app, vec![key_event(egui::Key::Escape)]);
    assert!(app.prompt().is_none());
    assert_eq!(app.title(), "v2");
    // ← でもう一度出し、はい
    frame(&mut app, vec![key_event(egui::Key::ArrowLeft)]);
    assert_eq!(app.prompt(), Some(NavDir::Prev));
    frame(&mut app, vec![key_event(egui::Key::Enter)]);
    assert_eq!(app.title(), "v1");
    // 最初の冊では候補なし
    frame(&mut app, vec![key_event(egui::Key::PageUp)]);
    let text = frame(&mut app, vec![key_event(egui::Key::Enter)]);
    assert_eq!(app.title(), "v1");
    assert!(text.contains("前の冊はありません"), "{text}");
}

#[test]
fn reverse_wheel_closes_prompt_without_paging() {
    let dir = TmpDir::new("nav-reverse");
    write_zip(&dir.0.join("v1.zip"), &["1.png", "2.png"]);
    write_zip(&dir.0.join("v2.zip"), &["1.png", "2.png"]);
    let mut app = app_for(&dir.0.join("v1.zip"));
    frame(&mut app, vec![wheel(-1.0)]);
    frame(&mut app, vec![wheel(-1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    // 同じ向き（下）は無視して出たまま
    frame(&mut app, vec![wheel(-1.0), wheel(-1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    // 逆向き（上）で閉じる。冊も頁もそのまま
    frame(&mut app, vec![wheel(1.0)]);
    assert!(app.prompt().is_none());
    let text = frame(&mut app, vec![]);
    assert!(!text.contains("次の冊を開きますか"), "{text}");
    assert_eq!((app.title(), app.page()), ("v1", 1));

    // 前冊の確認は下ホイールで閉じる
    frame(&mut app, vec![wheel(1.0)]);
    frame(&mut app, vec![wheel(1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Prev));
    frame(&mut app, vec![wheel(1.0)]);
    assert_eq!(app.prompt(), Some(NavDir::Prev));
    frame(&mut app, vec![wheel(-1.0)]);
    assert!(app.prompt().is_none());
    assert_eq!((app.title(), app.page()), ("v1", 0));
}

/// 同じ `Context` で 1 フレーム回し、描画された（文字列, 位置）を返す（クリック試験用）。
fn frame_ctx(
    ctx: &egui::Context,
    app: &mut ViewApp,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Pos2)> {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 800.0),
        )),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    out.textures_delta.clear();
    out.shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos)),
            _ => None,
        })
        .collect()
}

/// 確認のボタン `label` をマウスで押して離す。
fn click_button(ctx: &egui::Context, app: &mut ViewApp, label: &str) {
    let texts = frame_ctx(ctx, app, vec![]);
    let pos = texts
        .iter()
        .find(|(t, _)| t == label)
        .unwrap_or_else(|| panic!("{label} が描かれていない: {texts:?}"))
        .1
        + egui::vec2(8.0, 8.0);
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame_ctx(ctx, app, vec![egui::Event::PointerMoved(pos)]);
    frame_ctx(ctx, app, vec![button(true)]);
    frame_ctx(ctx, app, vec![button(false)]);
}

#[test]
fn prompt_buttons_answer_by_mouse_click() {
    let dir = TmpDir::new("nav-mouse");
    write_zip(&dir.0.join("v1.zip"), &["1.png"]);
    write_zip(&dir.0.join("v2.zip"), &["1.png"]);
    let mut app = app_for(&dir.0.join("v1.zip"));
    let ctx = egui::Context::default();
    frame_ctx(&ctx, &mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    // いいえ: 閉じて今の冊のまま
    click_button(&ctx, &mut app, "いいえ");
    assert!(app.prompt().is_none());
    assert_eq!(app.title(), "v1");
    // はい: 次の冊へ
    frame_ctx(&ctx, &mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    click_button(&ctx, &mut app, "はい");
    assert!(app.prompt().is_none());
    assert_eq!(app.title(), "v2");
}

#[test]
fn prompt_floats_with_yes_no_buttons() {
    let dir = TmpDir::new("nav-click");
    write_zip(&dir.0.join("v1.zip"), &["1.png"]);
    let mut app = app_for(&dir.0.join("v1.zip"));
    let text = frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert!(text.contains("はい") && text.contains("いいえ"), "{text}");
    assert_eq!(app.prompt(), Some(NavDir::Next));
}

#[test]
fn spread_prev_prompt_only_on_left_first_page() {
    let dir = TmpDir::new("nav-spread-first");
    write_zip(&dir.0.join("a.zip"), &["1.png", "2.png", "3.png", "4.png"]);
    let mut app = app_for(&dir.0.join("a.zip"));
    frame(&mut app, vec![key_event(egui::Key::F)]);
    frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    frame(&mut app, vec![key_event(egui::Key::ArrowLeft)]);
    assert_eq!(app.page(), 0);
    assert!(app.prompt().is_none());
    frame(&mut app, vec![key_event(egui::Key::ArrowLeft)]);
    assert_eq!(app.prompt(), Some(NavDir::Prev));
}

#[test]
fn index_order_is_used_when_db_is_given() {
    let dir = TmpDir::new("nav-db");
    let root = dir.0.join("lib");
    write_zip(&root.join("a.zip"), &["1.png"]);
    write_zip(&root.join("b.zip"), &["1.png"]);
    write_zip(&root.join("c.zip"), &["1.png"]);
    let db = dir.0.join("index.db");
    Index::open(&db).unwrap().reindex(&root).unwrap();

    let (title, book) = open_path(&root.join("a.zip")).unwrap();
    let mut app = ViewApp::new(title, book, 0, None).with_index(db, Some(root));
    frame(&mut app, vec![key_event(egui::Key::ArrowRight)]);
    assert_eq!(app.prompt(), Some(NavDir::Next));
    frame(&mut app, vec![key_event(egui::Key::Enter)]);
    assert_eq!(app.title(), "b");
}

#[test]
fn prompt_panel_fits_narrow_windows() {
    // 広い窓は既定の幅
    assert_eq!(PromptLayout::fit(1200.0).panel, PromptLayout::WIDE);
    for width in [560.0, 480.0, 400.0, 320.0, 260.0] {
        let l = PromptLayout::fit(width);
        assert!(
            l.panel <= width - PromptLayout::EDGE * 2.0,
            "{width}: {l:?}"
        );
        // ボタン 2 つと間が内側に収まる
        let inner = l.panel - l.margin * 2.0;
        assert!(l.button.x * 2.0 + l.gap <= inner + 0.01, "{width}: {l:?}");
        assert!(l.button.x >= 60.0, "{width}: {l:?}");
    }
}
