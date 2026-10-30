//! 画像ヘッダ検証（p0-4）の試験。壊れた画像は 422、正常画像は 200 + 正しい Content-Type。合成データのみ。

mod common;

use common::{
    TmpDir, gif_bytes, jpeg_bytes, png_bytes, real_gif_bytes, real_jpeg_bytes, real_webp_bytes,
    webp_bytes, write_zip_data,
};
use manga_viewer::imgcheck::{self, Kind};
use manga_viewer::index::Index;
use manga_viewer::server::{self, handle};

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

fn get(index: &Index, path: &str) -> server::Response {
    handle(
        index,
        None,
        &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
}

fn id_of(index: &Index, title: &str) -> i64 {
    index
        .books()
        .unwrap()
        .into_iter()
        .find(|r| r.title == title)
        .unwrap()
        .id
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
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let (a, b) = (id_of(&index, "a"), id_of(&index, "b"));

    // (冊, 頁, 期待: Some((Content-Type, 本体)) か 404)
    type Want<'a> = Option<(&'a str, &'a Vec<u8>)>;
    let cases: Vec<(i64, usize, Want)> = vec![
        (a, 0, Some(("image/png", &png))),
        (a, 1, None),
        (a, 2, Some(("image/gif", &gif))),
        (b, 0, None),
        (b, 1, Some(("image/jpeg", &jpg))),
        (b, 2, Some(("image/webp", &webp))),
        (b, 3, None),
        (b, 4, None),
    ];
    for (id, n, want) in cases {
        for route in ["page", "image"] {
            let r = get(&index, &format!("/{route}/{id}/{n}"));
            match &want {
                Some((ctype, body)) => {
                    assert_eq!(
                        (r.status, r.content_type),
                        (200, *ctype),
                        "/{route}/{id}/{n}"
                    );
                    assert_eq!(&r.body, *body, "/{route}/{id}/{n}");
                }
                None => {
                    assert_eq!(r.status, 422, "/{route}/{id}/{n}");
                    assert!(r.content_type.starts_with("text/html"));
                    assert!(String::from_utf8_lossy(&r.body).contains("この頁は表示できません"));
                }
            }
        }
    }
}

#[test]
fn image_route_is_inline_and_page_route_is_not() {
    let t = TmpDir::new("p0-4-inline");
    let root = t.0.join("books");
    write_zip_data(
        &root.join("a.zip"),
        &[("1.png", &png_bytes()), ("2.jpg", &real_jpeg_bytes())],
    );
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let a = id_of(&index, "a");
    let header = |r: &server::Response, k: &str| {
        r.headers
            .iter()
            .find(|(h, _)| *h == k)
            .map(|(_, v)| v.clone())
    };
    let img0 = get(&index, &format!("/image/{a}/0"));
    assert_eq!(
        header(&img0, "Content-Disposition").as_deref(),
        Some("inline; filename=\"1.png\"")
    );
    let img1 = get(&index, &format!("/image/{a}/1"));
    assert_eq!(
        header(&img1, "Content-Disposition").as_deref(),
        Some("inline; filename=\"2.jpg\"")
    );
    let page = get(&index, &format!("/page/{a}/0"));
    assert_eq!(header(&page, "Content-Disposition"), None);
    assert_eq!(
        header(&page, "X-Content-Type-Options").as_deref(),
        Some("nosniff")
    );
    let wire = String::from_utf8_lossy(&img0.to_bytes()[..200]).into_owned();
    assert!(wire.contains("Content-Disposition: inline; filename=\"1.png\"\r\n"));
    // 範囲外・不正
    assert_eq!(get(&index, &format!("/image/{a}/2")).status, 404);
    assert_eq!(get(&index, &format!("/image/{a}/-1")).status, 404);
    assert_eq!(get(&index, "/image/9999/0").status, 404);
}

/// 拡張子が嘘でも、Content-Type は中身（magic）で決まる。
#[test]
fn content_type_follows_magic_not_extension() {
    let t = TmpDir::new("p0-4-ext");
    let root = t.0.join("books");
    write_zip_data(&root.join("a.zip"), &[("1.jpg", &png_bytes())]);
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let a = id_of(&index, "a");
    let r = get(&index, &format!("/page/{a}/0"));
    assert_eq!((r.status, r.content_type), (200, "image/png"));
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
fn body_corrupt_image_is_422_with_view_wording() {
    // 署名・終端は正しいが IDAT だけ壊れた PNG: imgcheck は通るが復号できない（view と同じ結果になること）
    let mut png = png_bytes();
    let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
    for b in &mut png[idat..idat + 6] {
        *b = 0xAB;
    }
    assert!(manga_viewer::imgcheck::check(&png).is_ok());
    let t = TmpDir::new("p0-7c-corrupt");
    let root = t.0.join("books");
    write_zip_data(&root.join("a.zip"), &[("1.png", &png)]);
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let a = id_of(&index, "a");
    for route in ["page", "image"] {
        let r = get(&index, &format!("/{route}/{a}/0"));
        assert_eq!(r.status, 422, "{route}");
        let body = String::from_utf8_lossy(&r.body).into_owned();
        assert!(
            body.contains("この頁は表示できません: 復号できません"),
            "{body}"
        );
        assert!(
            r.to_bytes()
                .starts_with(b"HTTP/1.0 422 Unprocessable Entity")
        );
    }
}
