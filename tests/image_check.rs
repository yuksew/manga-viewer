//! 画像ヘッダ検証（p0-4）の試験。頁ごとに、読める頁は形式（magic）どおりに復号でき、壊れた頁は
//! view と同じ文言の Err になる。合成データのみ。（serve 経由の試験は #7 で serve ごと削除した）

mod common;

use common::{
    TmpDir, gif_bytes, jpeg_bytes, png_bytes, real_gif_bytes, real_jpeg_bytes, real_webp_bytes,
    webp_bytes, write_zip_data,
};
use manga_viewer::book::Book;
use manga_viewer::imgcheck::{self, Kind};
use manga_viewer::scan::BookKind;
use manga_viewer::view::load_page;

fn truncated(mut v: Vec<u8>, cut: usize) -> Vec<u8> {
    v.truncate(v.len() - cut);
    v
}

#[test]
fn check_accepts_each_format() {
    assert_eq!(imgcheck::check(&png_bytes()), Ok(Kind::Png));
    assert_eq!(imgcheck::check(&jpeg_bytes()), Ok(Kind::Jpeg));
    assert_eq!(imgcheck::check(&gif_bytes()), Ok(Kind::Gif));
    assert_eq!(imgcheck::check(&webp_bytes()), Ok(Kind::Webp));
}

#[test]
fn check_rejects_every_truncation_length() {
    for (name, full) in [
        ("png", png_bytes()),
        ("jpeg", jpeg_bytes()),
        ("gif", gif_bytes()),
        ("webp", webp_bytes()),
    ] {
        for len in 0..full.len() {
            assert!(
                imgcheck::check(&full[..len]).is_err(),
                "{name} を {len}/{} バイトで切っても通った",
                full.len()
            );
        }
    }
}

#[test]
fn check_rejects_garbage_and_bad_headers() {
    assert!(imgcheck::check(b"").is_err());
    assert!(imgcheck::check(b"not an image at all").is_err());
    // PNG: 幅 0
    let mut p = png_bytes();
    p[16..20].copy_from_slice(&0u32.to_be_bytes());
    assert!(imgcheck::check(&p).is_err());
    // PNG: IEND の後ろにごみ
    let mut p = png_bytes();
    p.push(0);
    assert!(imgcheck::check(&p).is_err());
    // JPEG: 寸法 0
    let mut j = jpeg_bytes();
    j[7..9].copy_from_slice(&0u16.to_be_bytes());
    assert!(imgcheck::check(&j).is_err());
    // JPEG: 後ろの 0 埋めは許す
    let mut j = jpeg_bytes();
    j.extend([0, 0]);
    assert_eq!(imgcheck::check(&j), Ok(Kind::Jpeg));
    // GIF: 幅 0
    let mut g = gif_bytes();
    g[6..8].copy_from_slice(&0u16.to_le_bytes());
    assert!(imgcheck::check(&g).is_err());
    // WebP: チャンク種別が違う
    let mut w = webp_bytes();
    w[12..16].copy_from_slice(b"XXXX");
    assert!(imgcheck::check(&w).is_err());
}

fn open(path: &std::path::Path) -> Book {
    Book::open(path, BookKind::Zip).unwrap()
}

/// 2 冊・複数頁。各頁を個別に検証する（良・壊れ・良 / 壊れ・良・良・壊れ・壊れ）。
#[test]
fn per_page_verdicts_across_two_books() {
    let t = TmpDir::new("p0-4");
    let root = t.0.join("books");
    // サーバは実復号するので、配信される側は本物の符号化済みバイトを使う
    let (png, jpg, gif, webp) = (
        png_bytes(),
        real_jpeg_bytes(),
        real_gif_bytes(),
        real_webp_bytes(),
    );
    let (png_cut, jpg_cut) = (truncated(png.clone(), 5), truncated(jpg.clone(), 2));
    let gif_cut = truncated(gif.clone(), 1);
    let webp_cut = truncated(webp.clone(), 3);
    write_zip_data(
        &root.join("a.zip"),
        &[("1.png", &png), ("2.png", &png_cut), ("3.gif", &gif)],
    );
    write_zip_data(
        &root.join("b.zip"),
        &[
            ("1.jpg", &jpg_cut),
            ("2.jpg", &jpg),
            ("3.webp", &webp),
            ("4.gif", &gif_cut),
            ("5.webp", &webp_cut),
        ],
    );
    let (a, b) = (open(&root.join("a.zip")), open(&root.join("b.zip")));

    // (冊, 頁, 期待: Some(形式) か 読めない)
    let cases: Vec<(&Book, usize, Option<Kind>)> = vec![
        (&a, 0, Some(Kind::Png)),
        (&a, 1, None),
        (&a, 2, Some(Kind::Gif)),
        // p0-8a: EOI だけ欠けた JPEG は読める所まで復号する（修復の印つき）
        (&b, 0, Some(Kind::Jpeg)),
        (&b, 1, Some(Kind::Jpeg)),
        (&b, 2, Some(Kind::Webp)),
        // GIF もトレーラ 1 バイト欠けなら全画素が読めるので同様に修復
        (&b, 3, Some(Kind::Gif)),
        (&b, 4, None),
    ];
    for (k, (book, n, want)) in cases.into_iter().enumerate() {
        match (want, load_page(book, n)) {
            (Some(kind), Ok(d)) => assert_eq!(d.kind, kind, "case {k}"),
            (None, Err(msg)) => assert!(!msg.is_empty(), "case {k}"),
            (want, got) => panic!("case {k}: want {want:?}, got {:?}", got.map(|d| d.kind)),
        }
    }
}

/// 拡張子が嘘でも、形式は中身（magic）で決まる。
#[test]
fn format_follows_magic_not_extension() {
    let t = TmpDir::new("p0-4-ext");
    let zip = t.0.join("books/a.zip");
    write_zip_data(&zip, &[("1.jpg", &png_bytes())]);
    assert_eq!(load_page(&open(&zip), 0).unwrap().kind, Kind::Png);
}

// ---- p0-4fix: 監査（p0-4）で生き残った変異 M05/M07/M09/M11/M17 を固定する試験 ----

/// M05: WebP は `RIFF` で始まらなければならない（`RIFX` 始まりは拒否）。
#[test]
fn webp_requires_riff_magic() {
    let mut w = webp_bytes();
    assert_eq!(imgcheck::check(&w), Ok(Kind::Webp));
    w[..4].copy_from_slice(b"RIFX");
    assert!(imgcheck::check(&w).is_err());
}

/// M07: JPEG の magic は先頭に限る。本体に `FF D8` を含む正常な GIF/WebP を JPEG と誤判定しない。
#[test]
fn ffd8_inside_body_does_not_make_a_jpeg() {
    // GIF: 画像データ（トレーラ 3B の手前）に FF D8
    let mut g = gif_bytes();
    let at = g.len() - 1;
    g.splice(at..at, [0xFF, 0xD8]);
    assert_eq!(imgcheck::check(&g), Ok(Kind::Gif));
    // WebP: ペイロード末尾に FF D8（RIFF・チャンク長も 2 バイト分広げる）
    let mut w = webp_bytes();
    w.extend([0xFF, 0xD8]);
    let riff = u32::from_le_bytes(w[4..8].try_into().unwrap()) + 2;
    w[4..8].copy_from_slice(&riff.to_le_bytes());
    let chunk = u32::from_le_bytes(w[16..20].try_into().unwrap()) + 2;
    w[16..20].copy_from_slice(&chunk.to_le_bytes());
    assert_eq!(imgcheck::check(&w), Ok(Kind::Webp));
}

/// M09: GIF は 13 バイトでは成立しない。13 バイトで末尾が `3B` のものも拒否する
/// （最小 GIF の 13 バイト接頭辞は末尾が 0 なので、全長の切り詰め試験では漏れる）。
#[test]
fn gif_of_13_bytes_ending_in_trailer_is_rejected() {
    let mut g = gif_bytes()[..12].to_vec();
    g.push(0x3B);
    assert_eq!(g.len(), 13);
    assert!(imgcheck::check(&g).is_err());
}

/// M11: WebP のチャンク長が RIFF の範囲を 1 バイトでも超えれば拒否する。
#[test]
fn webp_chunk_overrunning_riff_by_one_byte_is_rejected() {
    let mut w = webp_bytes();
    let chunk = u32::from_le_bytes(w[16..20].try_into().unwrap());
    w[16..20].copy_from_slice(&(chunk + 1).to_le_bytes());
    assert!(imgcheck::check(&w).is_err());
    // 境界ちょうど（チャンク長 = 実ペイロード長）は通る
    w[16..20].copy_from_slice(&chunk.to_le_bytes());
    assert_eq!(imgcheck::check(&w), Ok(Kind::Webp));
}

/// M17: PNG の IHDR 長さは 13 固定。14 にして本体を 1 バイト足し、後続チャンクの並びは保ったまま拒否を見る。
#[test]
fn png_ihdr_length_other_than_13_is_rejected() {
    let mut p = png_bytes();
    p[8..12].copy_from_slice(&14u32.to_be_bytes());
    p.insert(29, 0);
    assert!(imgcheck::check(&p).is_err());
}

#[test]
fn body_corrupt_image_is_a_decode_error() {
    // 署名・終端は正しいが IDAT だけ壊れた PNG: imgcheck は通るが復号できない
    let mut png = png_bytes();
    let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
    for b in &mut png[idat..idat + 6] {
        *b = 0xAB;
    }
    assert!(manga_viewer::imgcheck::check(&png).is_ok());
    let t = TmpDir::new("p0-7c-corrupt");
    let zip = t.0.join("books/a.zip");
    write_zip_data(&zip, &[("1.png", &png)]);
    let err = load_page(&open(&zip), 0).unwrap_err();
    assert!(err.contains("復号できません"), "{err}");
}
