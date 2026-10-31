//! #3: 設計 §2.1 の速さの合格線（2026-10-31 改訂）を合成データで測る。
//!
//! `cargo test --release --test perf_lines -- --ignored --nocapture --test-threads=1`
//!
//! 各テストは `BENCH <名前> <値>` を出す。合否で落とさない（数値を持ち帰るのが目的）。
//! 窓はヘッドレス egui で回すので、GPU へのテクスチャ転送と実フレームの表示は含まない
//! （CPU 側の「入力 → そのフレームで頁の絵が揃う」まで）。

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{TmpDir, write_zip, write_zip_data};
use eframe::egui;
use manga_viewer::book::Book;
use manga_viewer::index::Index;
use manga_viewer::library::Library;
use manga_viewer::scan::BookKind;
use manga_viewer::view::{ViewApp, open_path};

/// 設計の合成頁: ~1.4 MB PNG、約 1500×2200。漫画らしく白地に線と網点を置く（全面ノイズより現実寄り）。
fn manga_png(seed: u32) -> Vec<u8> {
    let (w, h) = (1500u32, 2200u32);
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let img = image::GrayImage::from_fn(w, h, |x, y| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let frame = x % 500 < 4 || y % 700 < 4;
        let tone = (x / 3 + y / 3) % 4 == 0 && (y / 350) % 2 == 1;
        let v: u8 = if frame || tone { 30 } else { 245 };
        image::Luma([v.saturating_sub((s & 7) as u8)])
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// p95 と max（ms）。
fn p95_max(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    let i = ((v.len() as f64 * 0.95).ceil() as usize).clamp(1, v.len()) - 1;
    (v[i], v[v.len() - 1])
}

fn zip_of(dir: &Path, name: &str, pages: &[Vec<u8>]) -> std::path::PathBuf {
    let names: Vec<String> = (0..pages.len()).map(|i| format!("{i:04}.png")).collect();
    let entries: Vec<(&str, &[u8])> = names
        .iter()
        .zip(pages)
        .map(|(n, p)| (n.as_str(), p.as_slice()))
        .collect();
    let zip = dir.join(name);
    write_zip_data(&zip, &entries);
    zip
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

fn run_frame(ctx: &egui::Context, app: &mut ViewApp, events: Vec<egui::Event>) -> Duration {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 1000.0),
        )),
        events,
        ..Default::default()
    };
    let t = Instant::now();
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    let d = t.elapsed();
    out.textures_delta.clear();
    d
}

/// 100 頁の冊（10 種の頁を繰り返す。zip の中身は全部別エントリ）。
fn book_100(dir: &Path) -> std::path::PathBuf {
    let base: Vec<Vec<u8>> = (1..=10).map(manga_png).collect();
    let pages: Vec<Vec<u8>> = (0..100).map(|i| base[i % 10].clone()).collect();
    println!("INFO page_png_kb {}", base[0].len() / 1024);
    zip_of(dir, "b100.zip", &pages)
}

/// B-1（先読み命中）と B-5: 100 ms 間隔で 100 頁送る。各送りフレームの所要と、巡回後の RSS。
#[test]
#[ignore = "実測用"]
fn b1_hit_and_b5_memory() {
    let dir = TmpDir::new("b1");
    let zip = book_100(&dir.0);
    let (title, book) = open_path(&zip).unwrap();
    let mut app = ViewApp::new(title, book, 0, None);
    let ctx = egui::Context::default();
    run_frame(&ctx, &mut app, vec![]);
    let mut turns = Vec::new();
    for _ in 0..99 {
        // 読んでいる間（100 ms）に先読みが進む。途中の再描画要求は取り込みフレームで処理される
        std::thread::sleep(Duration::from_millis(100));
        run_frame(&ctx, &mut app, vec![]);
        turns.push(ms(run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowRight)],
        )));
    }
    let (p95, max) = p95_max(turns);
    println!("BENCH b1_hit_p95_ms {p95:.1}");
    println!("BENCH b1_hit_max_ms {max:.1}");
    println!("BENCH b1_hit_sync_loads {}", app.sync_loads() - 1);
    let rss = memory_stats::memory_stats().map_or(0, |m| m.physical_mem) / (1 << 20);
    println!("BENCH b5_rss_after_100_pages_mb {rss}");
}

/// B-1'（先読み外れ）: 待たずに毎フレーム送る（先読みを追い越す）と、End/Home の跳び。
/// 部品: 1 頁ジョブ（バイト読み + 復号）単体 200 回。
#[test]
#[ignore = "実測用"]
fn b1_miss_and_job() {
    let dir = TmpDir::new("b1m");
    let zip = book_100(&dir.0);
    let (title, book) = open_path(&zip).unwrap();
    let mut app = ViewApp::new(title, book, 0, None);
    let ctx = egui::Context::default();
    run_frame(&ctx, &mut app, vec![]);
    let mut turns = Vec::new();
    for _ in 0..60 {
        turns.push(ms(run_frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::ArrowRight)],
        )));
    }
    let (p95, max) = p95_max(turns);
    println!("BENCH b1_miss_rapid_p95_ms {p95:.1}");
    println!("BENCH b1_miss_rapid_max_ms {max:.1}");

    let book = Book::open(&zip, BookKind::Zip).unwrap();
    let mut jobs = Vec::new();
    for i in 0..200 {
        let t = Instant::now();
        let bytes = book.read_page_bytes(i % 100).unwrap();
        let d = manga_viewer::decode::decode_rgba(&bytes).unwrap();
        jobs.push(ms(t.elapsed()));
        assert_eq!(d.width, 1500);
    }
    let (p95, max) = p95_max(jobs);
    println!("BENCH b1_job_p95_ms {p95:.1}");
    println!("BENCH b1_job_max_ms {max:.1}");

    // #8: 窓（900×1000）に収まるよう縮めて RGBA にするまで（表示が実際に使う経路）
    let mut fits = Vec::new();
    for i in 0..200 {
        let t = Instant::now();
        let bytes = book.read_page_bytes(i % 100).unwrap();
        let d = manga_viewer::decode::decode_fit(&bytes, 900, 1000).unwrap();
        fits.push(ms(t.elapsed()));
        assert_eq!(d.height, 1000);
    }
    let (p95, max) = p95_max(fits);
    println!("BENCH b1_job_fit_p95_ms {p95:.1}");
    println!("BENCH b1_job_fit_max_ms {max:.1}");
}

/// B-4: 冊を開く → 初頁が復号されテクスチャになるまで（28 頁の冊と 1000 エントリの冊）。
#[test]
#[ignore = "実測用"]
fn b4_open_first_page() {
    let dir = TmpDir::new("b4");
    let page = manga_png(1);
    let small = common::png_bytes();
    let z28 = zip_of(&dir.0, "b28.zip", &vec![page.clone(); 28]);
    let mut many = vec![small; 1000];
    many[0] = page;
    let z1000 = zip_of(&dir.0, "b1000.zip", &many);
    for (label, zip) in [("28", &z28), ("1000", &z1000)] {
        let mut runs = Vec::new();
        for _ in 0..10 {
            let ctx = egui::Context::default();
            let t = Instant::now();
            let (title, book) = open_path(zip).unwrap();
            let mut app = ViewApp::new(title, book, 0, None);
            run_frame(&ctx, &mut app, vec![]);
            runs.push(ms(t.elapsed()));
        }
        let (p95, max) = p95_max(runs);
        println!("BENCH b4_open_first_page_{label}_p95_ms {p95:.1}");
        println!("BENCH b4_open_first_page_{label}_max_ms {max:.1}");
    }
}

/// 724 冊（各 28 頁、頁は小さい）の root を作る。`cover` があれば各冊の先頭頁をそれにする。
fn root_724(dir: &Path, cover: Option<&[u8]>, n: usize) -> std::path::PathBuf {
    let root = dir.join("root");
    let names: Vec<String> = (1..=28).map(|i| format!("{i:03}.png")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    for b in 0..n {
        let path = root.join(format!("series{:03}/vol{b:04}.zip", b / 10));
        match cover {
            Some(cover) => {
                let small = common::png_bytes();
                let entries: Vec<(&str, &[u8])> = refs
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (*n, if i == 0 { cover } else { small.as_slice() }))
                    .collect();
                write_zip_data(&path, &entries);
            }
            None => write_zip(&path, &refs),
        }
    }
    root
}

/// B-3(a)(b): DB 無しから先頭 100 冊 / 全 724 冊の構造索引。B-2: 索引済み DB から一覧の初回描画まで。
#[test]
#[ignore = "実測用"]
fn b3_index_and_b2_list() {
    let dir = TmpDir::new("b3");
    let root = root_724(&dir.0, None, 724);
    // ウォームキャッシュ: 1 回捨てる
    Index::open(&dir.0.join("warm.db"))
        .unwrap()
        .reindex(&root)
        .unwrap();

    let t = Instant::now();
    let mut index = Index::open(&dir.0.join("a.db")).unwrap();
    index.reindex_upto(&root, Some(100)).unwrap();
    println!("BENCH b3a_index_first100_ms {:.1}", ms(t.elapsed()));

    let t = Instant::now();
    let mut index = Index::open(&dir.0.join("b.db")).unwrap();
    let report = index.reindex(&root).unwrap();
    println!("BENCH b3b_index_all_ms {:.1}", ms(t.elapsed()));
    assert_eq!(report.discovered, 724);
    drop(index);

    let db = dir.0.join("b.db");
    let mut runs = Vec::new();
    for _ in 0..5 {
        let ctx = egui::Context::default();
        let t = Instant::now();
        let library = Library::open(&db, Some(&root)).unwrap();
        let mut app = ViewApp::with_library(library);
        run_frame(&ctx, &mut app, vec![]);
        runs.push(ms(t.elapsed()));
    }
    let (p95, _) = p95_max(runs);
    println!("BENCH b2_db_to_first_list_frame_ms {p95:.1}");
}

/// B-3(c): 実寸表紙入り 100 冊の表紙生成。B-2 後半: 表紙のある 100 冊で、見えている表紙が埋まるまで。
#[test]
#[ignore = "実測用"]
fn b3c_covers_and_b2_visible_covers() {
    let dir = TmpDir::new("b3c");
    let cover = manga_png(7);
    let root = root_724(&dir.0, Some(&cover), 100);
    let db = dir.0.join("c.db");
    let mut index = Index::open(&db).unwrap();
    index.reindex(&root).unwrap();
    let t = Instant::now();
    let report = index.generate_covers(None, None).unwrap();
    println!("BENCH b3c_thumbs_100_ms {:.1}", ms(t.elapsed()));
    assert_eq!(report.generated, 100);
    drop(index);

    let ctx = egui::Context::default();
    let t = Instant::now();
    let library = Library::open(&db, Some(&root)).unwrap();
    let mut app = ViewApp::with_library(library);
    // 900×1000 の窓で見えるのは 4 列 × 4 行弱。テクスチャ数が止まるまで回す
    let mut last = usize::MAX;
    loop {
        run_frame(&ctx, &mut app, vec![]);
        let n = app.library().unwrap().cover_textures();
        if n == last {
            break;
        }
        last = n;
    }
    println!("BENCH b2_visible_covers_ms {:.1}", ms(t.elapsed()));
    println!("INFO b2_visible_covers_count {last}");
}

/// 実窓の通し（#3）: 合成 724 冊（先頭は 120 頁の実寸の冊、残りは中くらいの表紙 + 小さい頁）を索引・表紙生成し、
/// release の本体を `--autopilot` で起動する。GPU 転送・窓生成・実プロセスの RSS を含む。
/// `startup` を 3 回（1 回目は温め）と `reader` を 1 回。本体の `BENCH` 行に加え、プロセスの壁時計も出す。
#[test]
#[ignore = "実測用（実際に窓が開く）"]
fn real_window_autopilot() {
    let dir = TmpDir::new("rw");
    let root = dir.0.join("root");
    let base: Vec<Vec<u8>> = (1..=10).map(manga_png).collect();
    let pages: Vec<Vec<u8>> = (0..120).map(|i| base[i % 10].clone()).collect();
    zip_of(&root.join("series000"), "vol0000.zip", &pages);
    let cover = {
        let img = image::load_from_memory(&base[0])
            .unwrap()
            .thumbnail(750, 1100);
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    };
    let small = common::png_bytes();
    let names: Vec<String> = (1..=28).map(|i| format!("{i:03}.png")).collect();
    for b in 1..724 {
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), if i == 0 { &cover[..] } else { &small[..] }))
            .collect();
        let path = root.join(format!("series{:03}/vol{b:04}.zip", b / 10));
        write_zip_data(&path, &entries);
    }
    let db = dir.0.join("rw.db");
    let mut index = Index::open(&db).unwrap();
    assert_eq!(index.reindex(&root).unwrap().discovered, 724);
    let covers = index.generate_covers(None, None).unwrap();
    println!("INFO covers_generated {}", covers.generated);
    drop(index);

    for scene in ["startup", "startup", "startup", "reader"] {
        let t = Instant::now();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_manga-viewer"))
            .args(["--db".as_ref(), db.as_os_str()])
            .args(["--root".as_ref(), root.as_os_str()])
            .args(["--autopilot", scene])
            .output()
            .unwrap();
        let wall = ms(t.elapsed());
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        println!("== {scene}");
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            println!("{line}");
        }
        println!("BENCH process_wall_ms {wall:.0}");
    }
}
