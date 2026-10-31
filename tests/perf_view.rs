//! p0-14: 頁読み込みの実測（合成画像）。`cargo test --offline --test perf_view -- --ignored --nocapture`
//!
//! 1 頁あたりの ms を、読み出し / 復号 / ColorImage 変換 + load_texture（CPU 側）に分けて出す。
//! GPU への実アップロードは窓の描画時なのでここでは測れない（窓内の `page` 統計行で出す）。

mod common;

use std::time::Instant;

use common::{TmpDir, write_zip_data};
use eframe::egui;
use manga_viewer::view::open_path;

/// 疑似乱数でノイズを載せた 1600x2400 の灰色寄り画像（圧縮が効きにくい＝重い側）。
fn synth(format: image::ImageFormat, seed: u32) -> Vec<u8> {
    let (w, h) = (1400u32, 2000u32);
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let base = ((x / 8 + y / 8) % 2 * 120) as u8;
        let v = base.wrapping_add((s & 15) as u8);
        image::Rgb([v, v, v])
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, format).unwrap();
    buf.into_inner()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// 次頁へ送る 1 フレームの所要 ms を、先読み前（冷）と先読み後（温）で比べる。
#[test]
#[ignore = "実測用"]
fn page_turn_latency() {
    use manga_viewer::view::ViewApp;
    let dir = TmpDir::new("perf-turn");
    let pages: Vec<Vec<u8>> = (1..=6)
        .map(|s| synth(image::ImageFormat::Jpeg, s))
        .collect();
    let names: Vec<String> = (0..6).map(|i| format!("{i}.jpg")).collect();
    let entries: Vec<(&str, &[u8])> = names
        .iter()
        .zip(&pages)
        .map(|(n, p)| (n.as_str(), p.as_slice()))
        .collect();
    let zip = dir.0.join("b.zip");
    write_zip_data(&zip, &entries);
    let (title, book) = open_path(&zip).unwrap();
    let mut app = ViewApp::new(title, book, 0, None);
    let ctx = egui::Context::default();
    let frame = |app: &mut ViewApp, key: Option<egui::Key>| {
        let events = key
            .map(|key| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .into_iter()
            .collect();
        let raw = egui::RawInput {
            events,
            ..Default::default()
        };
        let t = Instant::now();
        let mut out = ctx.run_ui(raw, |ui| app.show(ui));
        out.textures_delta.clear();
        ms(t)
    };
    let first = frame(&mut app, None);
    // 先読みの完了を待つ（実機ではユーザーが読んでいる間に終わる）
    std::thread::sleep(std::time::Duration::from_millis(500));
    let warm = frame(&mut app, Some(egui::Key::ArrowRight));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let warm2 = frame(&mut app, Some(egui::Key::ArrowRight));
    println!(
        "初回表示 {first:.1} ms / 先読み後の頁送り {warm:.1} ms, {warm2:.1} ms（sync 復号 {} 回）",
        app.sync_loads()
    );
}

#[test]
#[ignore = "実測用"]
fn page_load_timing() {
    for (label, format) in [
        ("jpeg", image::ImageFormat::Jpeg),
        ("png", image::ImageFormat::Png),
    ] {
        let dir = TmpDir::new("perf");
        let pages: Vec<Vec<u8>> = (1..=4).map(|s| synth(format, s)).collect();
        let ext = if label == "jpeg" { "jpg" } else { "png" };
        let names: Vec<String> = (0..4).map(|i| format!("{i}.{ext}")).collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .zip(&pages)
            .map(|(n, p)| (n.as_str(), p.as_slice()))
            .collect();
        let zip = dir.0.join("b.zip");
        write_zip_data(&zip, &entries);
        let (_, book) = open_path(&zip).unwrap();
        let ctx = egui::Context::default();
        let (mut read, mut dec, mut tex) = (0.0, 0.0, 0.0);
        for i in 0..4 {
            let t = Instant::now();
            let bytes = book.read_page_bytes(i).unwrap();
            read += ms(t);
            let t = Instant::now();
            let d = manga_viewer::decode::decode_rgba(&bytes).unwrap();
            dec += ms(t);
            let t = Instant::now();
            let size = [d.width as usize, d.height as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, &d.rgba);
            let _ = ctx.load_texture("p", color, egui::TextureOptions::LINEAR);
            tex += ms(t);
        }
        println!(
            "{label}: {} KB/頁  read {:.1} ms  decode {:.1} ms  colorimage+load_texture {:.1} ms (1 頁平均)",
            pages[0].len() / 1024,
            read / 4.0,
            dec / 4.0,
            tex / 4.0
        );
    }
}
