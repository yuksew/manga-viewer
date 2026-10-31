//! p0-8a: 破損本体の自動修復（末尾ゴミの切り落とし・末尾欠けの部分復号）と、view の注記。

mod common;

use std::io::Cursor;

use common::{TmpDir, write_zip_data};
use eframe::egui;
use image::ImageFormat;
use manga_viewer::book::Book;
use manga_viewer::decode::{DecodeError, Repair, decode_rgba};
use manga_viewer::scan::BookKind;
use manga_viewer::view::ViewApp;

/// 縞と勾配のある 64x64 JPEG（走査データが十分長く、途中で切れても先頭側は読める）。
fn big_jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_fn(64, 64, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 8) as u8])
    });
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
    buf.into_inner()
}

fn small_png() -> Vec<u8> {
    common::png_bytes()
}

#[test]
fn jpeg_trailing_garbage_is_trimmed_and_flagged() {
    let good = big_jpeg();
    let expected = decode_rgba(&good).unwrap();
    assert!(expected.repair.is_none());

    let mut bad = good.clone();
    bad.extend_from_slice(b"garbage after EOI \x00\x01\x02 more");
    let d = decode_rgba(&bad).unwrap();
    assert_eq!(d.repair, Some(Repair::TrailingGarbage { len: good.len() }));
    assert_eq!(d.rgba, expected.rgba); // 画素は欠けていない
}

#[test]
fn png_trailing_garbage_is_trimmed_and_flagged() {
    let good = small_png();
    let mut bad = good.clone();
    bad.extend_from_slice(&[0xAA; 37]);
    let d = decode_rgba(&bad).unwrap();
    assert_eq!(d.repair, Some(Repair::TrailingGarbage { len: good.len() }));
    assert_eq!(d.rgba, decode_rgba(&good).unwrap().rgba);
}

#[test]
fn jpeg_cut_tail_is_decoded_partially_and_flagged() {
    let good = big_jpeg();
    let expected = decode_rgba(&good).unwrap();
    for n in [2, 30, good.len() / 8] {
        let d = decode_rgba(&good[..good.len() - n]).unwrap_or_else(|e| panic!("n={n}: {e}"));
        assert_eq!(d.repair, Some(Repair::Truncated), "n={n}");
        assert_eq!((d.width, d.height), (64, 64));
        // 先頭の行は元と一致（欠けるのは後ろ側）
        assert_eq!(d.rgba[..64 * 4], expected.rgba[..64 * 4], "n={n}");
    }
}

#[test]
fn head_side_damage_is_still_an_error() {
    let good = big_jpeg();
    // SOF0（FFC0）のマーカーを潰す → 寸法も復号表も読めない
    let sof = good.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let mut bad = good.clone();
    bad[sof + 1] = 0x01;
    assert!(decode_rgba(&bad).is_err());
    // 先頭 2 バイト（SOI）が壊れていれば形式判定で断る
    let mut bad = good.clone();
    bad[0] = 0;
    assert!(matches!(decode_rgba(&bad), Err(DecodeError::Invalid(_))));
    // PNG の IHDR 破損（末尾ゴミ付きでも直らない）
    let mut png = small_png();
    png[12] = b'X';
    png.extend_from_slice(b"junk");
    assert!(decode_rgba(&png).is_err());
}

#[test]
fn intact_images_have_no_repair_mark() {
    assert!(decode_rgba(&small_png()).unwrap().repair.is_none());
    assert!(decode_rgba(&big_jpeg()).unwrap().repair.is_none());
}

/// 1 フレーム回して描画された全文字列を返す。
fn frame_text(app: &mut ViewApp, key: Option<egui::Key>) -> String {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    if let Some(key) = key {
        raw.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    }
    let mut out = ctx.run_ui(raw, |ui| app.show(ui));
    let mut text = String::new();
    for clipped in &out.shapes {
        collect_text(&clipped.shape, &mut text);
    }
    out.textures_delta.clear();
    text
}

fn collect_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

#[test]
fn view_shows_repair_note_only_on_repaired_page() {
    let mut garbage = big_jpeg();
    garbage.extend_from_slice(b"tail junk");
    let ok = small_png();
    let dir = TmpDir::new("repair-view");
    let zip = dir.0.join("b.zip");
    write_zip_data(&zip, &[("1.jpg", &garbage), ("2.png", &ok)]);
    let book = Book::open(&zip, BookKind::Zip).unwrap();

    let mut app = ViewApp::new("t".into(), book, 0, None);
    let text = frame_text(&mut app, None);
    assert!(text.contains("修復して表示"), "{text}");
    assert!(!text.contains("表示できません"), "{text}");

    let text = frame_text(&mut app, Some(egui::Key::ArrowRight));
    assert!(text.contains("2/2"), "{text}");
    assert!(!text.contains("修復して表示"), "{text}");
}

#[test]
fn view_still_errors_on_unrepairable_page() {
    let mut bad = big_jpeg();
    let sof = bad.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    bad[sof + 1] = 0x01;
    let dir = TmpDir::new("repair-view-err");
    let zip = dir.0.join("b.zip");
    write_zip_data(&zip, &[("1.jpg", &bad)]);
    let book = Book::open(&zip, BookKind::Zip).unwrap();
    let mut app = ViewApp::new("t".into(), book, 0, None);
    let text = frame_text(&mut app, None);
    assert!(text.contains("この頁は表示できません"), "{text}");
    assert!(!text.contains("修復して表示"), "{text}");
}

/// 修復の種類（serve の X-MV-Repaired ヘッダで見ていた区別。#7 で serve を消したので decode で直接見る）。
#[test]
fn repair_kinds_per_page() {
    let good = big_jpeg();
    let mut garbage = good.clone();
    garbage.extend_from_slice(b"tail junk");
    let cut = good[..good.len() - 30].to_vec();
    let mut bad_head = good.clone();
    let sof = bad_head.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    bad_head[sof + 1] = 0x01;

    assert_eq!(decode_rgba(&good).unwrap().repair, None);
    assert_eq!(
        decode_rgba(&garbage).unwrap().repair,
        Some(Repair::TrailingGarbage { len: good.len() })
    );
    assert_eq!(decode_rgba(&cut).unwrap().repair, Some(Repair::Truncated));
    assert!(decode_rgba(&bad_head).is_err());
}

/// 順序の固定: ヘッダ内（APP15 の中身）に `FFD9` があっても、そこでは切らず真の終端で切る。
#[test]
fn jpeg_eoi_bytes_inside_header_segment_do_not_win() {
    let good = big_jpeg();
    let payload = b"thumb\xFF\xD9end";
    let seg_len = (payload.len() + 2) as u16;
    let mut with_seg = good[..2].to_vec();
    with_seg.extend_from_slice(&[0xFF, 0xEF]);
    with_seg.extend_from_slice(&seg_len.to_be_bytes());
    with_seg.extend_from_slice(payload);
    with_seg.extend_from_slice(&good[2..]);
    let mut bad = with_seg.clone();
    bad.extend_from_slice(b"trailing junk");
    let d = decode_rgba(&bad).unwrap();
    assert_eq!(
        d.repair,
        Some(Repair::TrailingGarbage {
            len: with_seg.len()
        })
    );
}
