//! #20: 見開きで横長の頁は並べず 1 枚で出し、その次から組み直す。

mod common;

use std::io::Write;
use std::time::{Duration, Instant};

use common::TmpDir;
use eframe::egui;
use manga_viewer::view::{
    Layout, Nav, PageSet, ViewApp, mode_label, open_path, plan_nav, spread_pages,
};

#[test]
fn wide_page_breaks_pairs() {
    // 5 頁目（索引 4）が横長の 8 頁
    let wide = [false, false, false, false, true, false, false, false];
    let pages = PageSet {
        count: 8,
        wide: &wide,
    };
    let groups = |layout: Layout| {
        let mut out = Vec::new();
        let mut p = 0;
        while p < 8 {
            let g = spread_pages(p, layout, pages);
            out.push(g);
            p = g.1 + 1;
        }
        out
    };
    // 表紙 1 枚: 1 | 2-3 | 4 | 5(横長) | 6-7 | 8（4 頁目は相手が横長なので 1 枚）
    assert_eq!(
        groups(Layout::Spread),
        [(0, 0), (1, 2), (3, 3), (4, 4), (5, 6), (7, 7)]
    );
    // 組ずらし: 1-2 | 3-4 | 5(横長) | 6-7 | 8
    assert_eq!(
        groups(Layout::Shifted),
        [(0, 1), (2, 3), (4, 4), (5, 6), (7, 7)]
    );
    // 単ページは変わらない
    assert_eq!(spread_pages(4, Layout::Single, pages), (4, 4));
    // 送りも組に従う
    assert_eq!(plan_nav(3, 1, Layout::Spread, pages), Nav::Page(4));
    assert_eq!(plan_nav(4, 1, Layout::Spread, pages), Nav::Page(5));
    assert_eq!(plan_nav(5, -1, Layout::Spread, pages), Nav::Page(4));
    assert!(mode_label(true, 1000.0, Layout::Spread, 4, pages).contains("横長のページは 1 枚"));
    assert!(mode_label(true, 1000.0, Layout::Spread, 3, pages).contains("次が横長のため 1 枚"));
    assert!(mode_label(true, 1000.0, Layout::Spread, 7, pages).contains("最後のページは 1 枚"));
    // 寸法が分からなければ（頁数だけ）従来どおり
    assert_eq!(spread_pages(3, Layout::Spread, 8), (3, 4));
}

/// 頁ごとに寸法の違う PNG を入れた zip。
fn zip_with(dir: &TmpDir, sizes: &[(u32, u32)]) -> std::path::PathBuf {
    let zip = dir.0.join("a.zip");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&zip).unwrap());
    for (i, &(width, height)) in sizes.iter().enumerate() {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([200, 100, 50]));
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
    zip
}

struct Harness {
    ctx: egui::Context,
    app: ViewApp,
}

impl Harness {
    /// 1 フレーム回し、(上部の表示, 頁画像の矩形) を返す。
    fn frame(&mut self, events: Vec<egui::Event>) -> (String, Vec<egui::Rect>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.show(ui));
        out.textures_delta.clear();
        let rects = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                    Some(m.calc_bounds())
                }
                _ => None,
            })
            .collect();
        (self.app.status().to_string(), rects)
    }

    fn key(&mut self, k: egui::Key) -> (String, Vec<egui::Rect>) {
        self.frame(vec![egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        self.frame(vec![])
    }
}

#[test]
fn wide_page_is_shown_alone_in_spread() {
    let dir = TmpDir::new("wide-view");
    let (tall, wide) = ((60, 90), (180, 90));
    let zip = zip_with(&dir, &[tall, tall, tall, wide, tall, tall]);
    let (title, book) = open_path(&zip).unwrap();
    let mut h = Harness {
        ctx: egui::Context::default(),
        app: ViewApp::new(title, book, 0, None),
    };
    h.key(egui::Key::F);
    // 裏の寸法調べが届くまで回す（2-3 の組の後に 4 頁目が 1 枚になるかで見る）
    let t = Instant::now();
    loop {
        h.frame(vec![]);
        std::thread::sleep(Duration::from_millis(5));
        let (s, _) = h.key(egui::Key::ArrowRight);
        assert!(s.starts_with("2-3/6"), "{s}");
        let (s, rects) = h.key(egui::Key::ArrowRight);
        if s.starts_with("4/6") {
            assert!(s.contains("横長のページは 1 枚"), "{s}");
            // 横長の頁は窓の幅いっぱいに使える（半分に縮めない）
            assert_eq!(rects.len(), 1, "{rects:?}");
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "寸法調べが届かない: {s}"
        );
        h.key(egui::Key::ArrowLeft);
        h.key(egui::Key::ArrowLeft);
    }
    let (s, _) = h.key(egui::Key::ArrowRight);
    assert!(s.starts_with("5-6/6"), "{s}");
    let (s, _) = h.key(egui::Key::ArrowLeft);
    assert!(s.starts_with("4/6"), "{s}");
}
