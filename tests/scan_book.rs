mod common;

use std::collections::BTreeSet;

use common::{DenyList, TmpDir, write_folder, write_zip};
use manga_viewer::book::{Book, BookError};
use manga_viewer::scan::{BookKind, scan_root, scan_root_full};

fn titles(root: &std::path::Path) -> BTreeSet<String> {
    scan_root(root)
        .unwrap()
        .into_iter()
        .map(|b| b.title)
        .collect()
}

#[test]
fn discovers_zip_and_folder_books_within_depth() {
    let t = TmpDir::new("scan");
    let r = &t.0;
    write_zip(&r.join("a/vol1.zip"), &["001.png"]); // 深さ2の zip
    write_zip(&r.join("loose.cbz"), &["1.png"]); // ルート直下の zip
    write_folder(&r.join("f1"), &["1.png", "2.jpg"]); // 深さ1のフォルダ冊
    write_folder(&r.join("g/h"), &["1.png"]); // 深さ2のフォルダ冊
    write_folder(&r.join("日本語の本"), &["001.png"]);
    std::fs::create_dir_all(r.join("empty")).unwrap(); // 空フォルダ
    write_folder(&r.join("txtonly"), &[]);
    std::fs::write(r.join("txtonly/readme.txt"), b"x").unwrap();
    write_zip(&r.join("d1/d2/d3/deep.zip"), &["1.png"]); // 深さ4: 拾わない
    write_folder(&r.join("d1/d2/d3b"), &["1.png"]); // 深さ4: 拾わない

    let want: BTreeSet<String> = ["vol1", "loose", "f1", "h", "日本語の本"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(titles(r), want);

    let kinds: Vec<_> = scan_root(r)
        .unwrap()
        .into_iter()
        .map(|b| (b.title, b.kind))
        .collect();
    assert!(kinds.contains(&("vol1".into(), BookKind::Zip)));
    assert!(kinds.contains(&("f1".into(), BookKind::Folder)));
}

#[test]
fn depth_three_book_is_not_picked_up() {
    let t = TmpDir::new("depth3");
    // root/a/b/c.zip は深さ3の位置（root=0, a=1, b=2 の直下）
    write_zip(&t.0.join("a/b/c.zip"), &["1.png"]);
    write_folder(&t.0.join("a/b/c"), &["1.png"]);
    let got = titles(&t.0);
    // 深さ2のディレクトリ b の直下にある zip は拾う。b/c フォルダ（深さ3）は拾わない。
    assert_eq!(got, BTreeSet::from(["c".to_string()]));
    let books = scan_root(&t.0).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].kind, BookKind::Zip);
}

#[test]
fn folder_and_zip_in_same_dir_are_two_books() {
    let t = TmpDir::new("mixed");
    write_folder(&t.0.join("m"), &["1.png"]);
    write_zip(&t.0.join("m/x.zip"), &["1.png"]);
    assert_eq!(scan_root(&t.0).unwrap().len(), 2);
}

#[test]
fn pages_are_in_natural_order_and_filtered() {
    let t = TmpDir::new("order");
    let z = t.0.join("b/book.zip");
    write_zip(
        &z,
        &[
            "10.png",
            "2.png",
            "1.PNG",
            "__MACOSX/1.png",
            ".hidden.png",
            "Thumbs.db",
            "notes.txt",
            "sub/3.png",
        ],
    );
    let book = Book::open(&z, BookKind::Zip).unwrap();
    assert_eq!(book.page_names(), ["1.PNG", "2.png", "10.png", "sub/3.png"]);
    assert!(!book.read_page_bytes(0).unwrap().is_empty());
    assert!(matches!(
        book.read_page_bytes(99),
        Err(BookError::PageOutOfRange(99))
    ));

    write_folder(&t.0.join("fol"), &["10.png", "2.png", "1.png"]);
    std::fs::write(t.0.join("fol/x.txt"), b"x").unwrap();
    let book = Book::open(&t.0.join("fol"), BookKind::Folder).unwrap();
    assert_eq!(book.page_names(), ["1.png", "2.png", "10.png"]);
    assert!(!book.read_page_bytes(2).unwrap().is_empty());
}

#[test]
fn broken_books_are_listed_but_fail_to_open() {
    let t = TmpDir::new("broken");
    let r = &t.0;
    write_zip(&r.join("empty.zip"), &[]);
    write_zip(&r.join("noimg.zip"), &["a.txt"]);
    let trunc = r.join("trunc.zip");
    write_zip(&trunc, &["1.png", "2.png"]);
    let bytes = std::fs::read(&trunc).unwrap();
    std::fs::write(&trunc, &bytes[..bytes.len() / 2]).unwrap();
    std::fs::write(r.join("garbage.zip"), b"not a zip").unwrap();

    let books = scan_root(r).unwrap();
    assert_eq!(books.len(), 4, "壊れた冊も一覧に出る");
    for b in &books {
        assert!(
            Book::open(&b.path, b.kind).is_err(),
            "{} は開けないはず",
            b.title
        );
    }
    let open_err = |t: &str| {
        let b = books.iter().find(|b| b.title == t).unwrap();
        Book::open(&b.path, b.kind).err().unwrap()
    };
    assert!(matches!(open_err("empty"), BookError::NoPages));
    assert!(matches!(open_err("noimg"), BookError::NoPages));
    assert!(matches!(open_err("trunc"), BookError::Zip(_)));
    assert!(matches!(open_err("garbage"), BookError::Zip(_)));
}

#[test]
fn thousand_entry_zip_opens() {
    let t = TmpDir::new("big");
    let names: Vec<String> = (1..=1000).map(|i| format!("{i:04}.png")).collect();
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let z = t.0.join("big.zip");
    write_zip(&z, &refs);
    let book = Book::open(&z, BookKind::Zip).unwrap();
    assert_eq!(book.page_count(), 1000);
    assert_eq!(book.page_names()[999], "1000.png");
}

#[test]
fn cli_index_summarizes() {
    let t = TmpDir::new("cli");
    write_zip(&t.0.join("a/one.zip"), &["1.png", "2.png"]);
    write_folder(&t.0.join("f"), &["1.png"]);
    write_zip(&t.0.join("bad.zip"), &[]);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_manga-viewer"))
        .arg("index")
        .arg(&t.0)
        .arg("--db")
        .arg(t.0.join("db-out/index.db"))
        .env_remove("MANGA_VIEWER_DATA")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("books=3 pages=3 broken=1"), "{s}");
}

/// #5: 読めない子ディレクトリは飛ばして走査を続け、飛ばしたことを理由つきで返す。
#[test]
fn unreadable_subdir_is_reported_not_silently_dropped() {
    let t = TmpDir::new("scan-deny");
    let r = &t.0;
    write_zip(&r.join("ok/vol1.zip"), &["1.png"]);
    write_zip(&r.join("locked/vol2.zip"), &["1.png"]);
    let _deny = DenyList::new(&r.join("locked"));

    let scan = scan_root_full(r).unwrap();
    let got: Vec<_> = scan.books.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(got, ["vol1"]);
    assert_eq!(scan.skipped.len(), 1);
    assert_eq!(scan.skipped[0].path, r.join("locked"));
    assert!(!scan.skipped[0].error.is_empty());
    // 互換の scan_root は冊だけを返す
    assert_eq!(scan_root(r).unwrap().len(), 1);
}

/// 読めない子ディレクトリが無ければ skipped は空。
#[test]
fn readable_tree_has_no_skipped() {
    let t = TmpDir::new("scan-noskip");
    write_zip(&t.0.join("a/v.zip"), &["1.png"]);
    assert!(scan_root_full(&t.0).unwrap().skipped.is_empty());
}

/// CP932（Shift_JIS）で書く。テスト用に encoding_rs で作る。
fn sjis(s: &str) -> Vec<u8> {
    let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(s);
    assert!(!had_errors);
    bytes.into_owned()
}

/// #6: UTF-8 フラグなしの CP932 名の頁を文字化けさせず、自然順で並べる。
/// 「表」は 2 バイト目が 0x5C（`\`）なので、区切りの置換を復号より先にすると壊れる。
#[test]
fn cp932_entry_names_are_decoded_and_sorted_naturally() {
    let t = TmpDir::new("cp932");
    let path = t.0.join("old.cbz");
    let png = common::png_bytes();
    let names = ["第10話.png", "第2話.png", "表紙/表1.png"];
    let raw: Vec<Vec<u8>> = names.iter().map(|n| sjis(n)).collect();
    // 前提: どれも UTF-8 としては不正（CP437 で読まれると化ける）
    assert!(raw.iter().all(|r| std::str::from_utf8(r).is_err()));
    let entries: Vec<(&[u8], &[u8])> = raw.iter().map(|r| (r.as_slice(), png.as_slice())).collect();
    common::write_zip_raw_names(&path, &entries);

    let book = Book::open(&path, BookKind::Zip).unwrap();
    assert_eq!(
        book.page_names(),
        ["第2話.png", "第10話.png", "表紙/表1.png"]
    );
    for i in 0..book.page_count() {
        assert_eq!(book.read_page_bytes(i).unwrap(), png);
    }
}

/// UTF-8 の名前はそのまま（CP932 として読み直さない）。
#[test]
fn utf8_entry_names_are_kept_as_utf8() {
    let t = TmpDir::new("utf8-raw");
    let path = t.0.join("new.cbz");
    let png = common::png_bytes();
    common::write_zip_raw_names(&path, &[("第1話.png".as_bytes(), png.as_slice())]);
    let book = Book::open(&path, BookKind::Zip).unwrap();
    assert_eq!(book.page_names(), ["第1話.png"]);
}

/// UTF-8 でも CP932 でも読めない名前は従来どおり zip クレートの解釈（CP437）で開ける。
#[test]
fn names_invalid_in_both_fall_back_without_error() {
    let t = TmpDir::new("cp437");
    let path = t.0.join("odd.cbz");
    let png = common::png_bytes();
    common::write_zip_raw_names(&path, &[(&[0xFF, b'.', b'p', b'n', b'g'], png.as_slice())]);
    let book = Book::open(&path, BookKind::Zip).unwrap();
    assert_eq!(book.page_count(), 1);
}
