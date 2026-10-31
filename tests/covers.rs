//! 表紙サムネイル（設計 p0-2c、#2）。検査データは合成のみ。

mod common;

use std::fs;

use common::{TmpDir, write_zip, write_zip_data};
use manga_viewer::cover::{MAX_H, MAX_W, cover_from_bytes};
use manga_viewer::index::Index;

/// 縦長の大きめの PNG（表紙が縮小される側）。
fn tall_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
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

#[test]
fn cover_is_jpeg_within_bounds_and_keeps_aspect() {
    let cover = cover_from_bytes(&tall_png(700, 1000)).unwrap();
    assert!(cover.jpeg.starts_with(&[0xFF, 0xD8]), "JPEG ではない");
    assert_eq!((cover.width, cover.height), (238, 340));
    let back = image::load_from_memory(&cover.jpeg).unwrap();
    assert_eq!((back.width(), back.height()), (cover.width, cover.height));
    assert!(cover.width <= MAX_W && cover.height <= MAX_H);
}

#[test]
fn covers_are_generated_once_and_broken_books_are_skipped() {
    let t = TmpDir::new("covers");
    let root = t.0.join("root");
    let big = tall_png(700, 1000);
    write_zip_data(
        &root.join("a/vol1.zip"),
        &[("001.png", &big), ("002.png", &big)],
    );
    write_zip(&root.join("b/vol2.zip"), &["001.png"]);
    fs::create_dir_all(root.join("c")).unwrap();
    fs::write(root.join("c/broken.zip"), b"not a zip").unwrap();
    // 開けるが先頭頁が壊れている冊
    write_zip_data(
        &root.join("d/badcover.zip"),
        &[("001.png", b"\x89PNG\r\n\x1a\nxx")],
    );

    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let first = index.generate_covers(Some(&root), None).unwrap();
    // broken.zip は status=broken なので対象外。badcover は試して失敗
    assert_eq!((first.missing, first.generated, first.failed), (3, 2, 1));

    let vol1 = index.cover(id_of(&index, "vol1")).unwrap().unwrap();
    assert_eq!((vol1.width, vol1.height), (238, 340));
    let vol2 = index.cover(id_of(&index, "vol2")).unwrap().unwrap();
    assert_eq!((vol2.width, vol2.height), (4, 4)); // 小さい画像は拡大しない
    assert!(index.cover(id_of(&index, "broken")).unwrap().is_none());
    assert!(index.cover(id_of(&index, "badcover")).unwrap().is_none());

    // 2 回目は作れなかった冊も含めて 0 件
    let second = index.generate_covers(Some(&root), None).unwrap();
    assert_eq!((second.missing, second.generated, second.failed), (0, 0, 0));
}

#[test]
fn limit_takes_books_in_natural_order() {
    let t = TmpDir::new("covers-limit");
    let root = t.0.join("root");
    for name in ["v10", "v2", "v1"] {
        write_zip(&root.join(format!("{name}/{name}.zip")), &["001.png"]);
    }
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let report = index.generate_covers(None, Some(2)).unwrap();
    assert_eq!((report.missing, report.generated), (3, 2));
    assert!(index.cover(id_of(&index, "v1")).unwrap().is_some());
    assert!(index.cover(id_of(&index, "v2")).unwrap().is_some());
    assert!(index.cover(id_of(&index, "v10")).unwrap().is_none());
}

/// 冊が消えると covers の行も消える（ON DELETE CASCADE。p0-2fix の N7 を固定）。
#[test]
fn deleting_a_book_cascades_to_its_cover() {
    let t = TmpDir::new("covers-cascade");
    let root = t.0.join("root");
    write_zip(&root.join("a/keep.zip"), &["001.png"]);
    write_zip(&root.join("b/gone.zip"), &["001.png"]);
    let db = t.0.join("db/index.db");
    let mut index = Index::open(&db).unwrap();
    index.reindex(&root).unwrap();
    index.generate_covers(None, None).unwrap();
    let gone = id_of(&index, "gone");

    fs::remove_file(root.join("b/gone.zip")).unwrap();
    let report = index.reindex(&root).unwrap();
    assert_eq!(report.removed, 1);
    drop(index);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM covers WHERE book_id = ?1",
            [gone],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "消えた冊の表紙が残っている");
    let all: i64 = conn
        .query_row("SELECT count(*) FROM covers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(all, 1);
}

/// 中身が変わった冊は表紙を作り直す（古い表紙を出さない）。
#[test]
fn updated_book_gets_a_fresh_cover() {
    let t = TmpDir::new("covers-update");
    let root = t.0.join("root");
    let zip = root.join("a/vol.zip");
    write_zip(&zip, &["001.png"]);
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    index.generate_covers(None, None).unwrap();
    assert_eq!(index.cover(id_of(&index, "vol")).unwrap().unwrap().width, 4);

    write_zip_data(&zip, &[("001.png", &tall_png(700, 1000))]);
    let report = index.reindex(&root).unwrap();
    assert_eq!(report.updated, 1);
    assert!(index.cover(id_of(&index, "vol")).unwrap().is_none());
    let again = index.generate_covers(None, None).unwrap();
    assert_eq!(again.generated, 1);
    assert_eq!(
        index.cover(id_of(&index, "vol")).unwrap().unwrap().width,
        238
    );
}
