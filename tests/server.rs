//! HTTP サーバ（p0-3）の試験。ハンドラ関数の単体と、実ソケットの統合 1 本。合成データのみ。

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use common::{TmpDir, png_bytes, write_folder, write_zip};
use manga_viewer::index::Index;
use manga_viewer::server::{self, content_type, handle};

/// a.zip（3 頁）・b.zip（1 頁・jpg）・f（フォルダ 2 頁）を索引した DB を返す。
fn setup(t: &TmpDir) -> (Index, PathBuf) {
    let root = t.0.join("books");
    write_zip(&root.join("a.zip"), &["1.png", "2.PNG", "10.png"]);
    write_zip(&root.join("b.zip"), &["x.jpg"]);
    write_folder(&root.join("f"), &["1.png", "2.png"]);
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    (index, root)
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

fn get(index: &Index, path: &str) -> server::Response {
    handle(
        index,
        None,
        &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
}

fn text(r: &server::Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

#[test]
fn content_type_by_extension() {
    assert_eq!(content_type("a.png"), Some("image/png"));
    assert_eq!(content_type("a.PNG"), Some("image/png"));
    assert_eq!(content_type("a.jpg"), Some("image/jpeg"));
    assert_eq!(content_type("a.JPEG"), Some("image/jpeg"));
    assert_eq!(content_type("a.webp"), Some("image/webp"));
    assert_eq!(content_type("a.gif"), Some("image/gif"));
    assert_eq!(content_type("a.bmp"), None);
    assert_eq!(content_type("png"), None);
}

#[test]
fn list_page_has_links_in_natural_order() {
    let t = TmpDir::new("srv-list");
    let (index, _) = setup(&t);
    let r = get(&index, "/");
    assert_eq!(r.status, 200);
    assert!(r.content_type.starts_with("text/html"));
    let html = text(&r);
    let pos = |title: &str| {
        html.find(&format!("/book/{}\"", id_of(&index, title)))
            .unwrap()
    };
    assert!(pos("a") < pos("b") && pos("b") < pos("f"));
    assert!(html.contains("3 ページ"));
}

#[test]
fn book_page_navigation_boundaries() {
    let t = TmpDir::new("srv-nav");
    let (index, _) = setup(&t);
    let id = id_of(&index, "a");
    // 先頭: prev なし・next あり
    let first = text(&get(&index, &format!("/book/{id}")));
    assert!(first.contains("1/3 ページ"));
    assert!(!first.contains(">prev<"));
    assert!(first.contains(&format!("/book/{id}?p=1\">next<")));
    assert!(first.contains(&format!("/page/{id}/0\"")));
    // 末尾 (M-1): prev あり・next なし
    let last = text(&get(&index, &format!("/book/{id}?p=2")));
    assert!(last.contains("3/3 ページ"));
    assert!(last.contains(&format!("/book/{id}?p=1\">prev<")));
    assert!(!last.contains(">next<"));
    // M は範囲外
    assert_eq!(get(&index, &format!("/book/{id}?p=3")).status, 404);
    assert_eq!(get(&index, &format!("/book/{id}?p=-1")).status, 404);
}

#[test]
fn page_bytes_and_range() {
    let t = TmpDir::new("srv-page");
    let (index, _) = setup(&t);
    let a = id_of(&index, "a");
    for n in [0, 2] {
        let r = get(&index, &format!("/page/{a}/{n}"));
        assert_eq!((r.status, r.content_type), (200, "image/png"));
        assert_eq!(r.body, png_bytes());
    }
    assert_eq!(get(&index, &format!("/page/{a}/3")).status, 404);
    assert_eq!(get(&index, &format!("/page/{a}/-1")).status, 404);
    assert_eq!(
        get(&index, &format!("/page/{a}/99999999999999999999")).status,
        404
    );
    // Content-Type は中身（magic）で決まる: 拡張子 jpg でも中身は PNG なので image/png（p0-4）
    let b = id_of(&index, "b");
    assert_eq!(
        get(&index, &format!("/page/{b}/0")).content_type,
        "image/png"
    );
    // フォルダの冊
    let f = id_of(&index, "f");
    assert_eq!(get(&index, &format!("/page/{f}/1")).body, png_bytes());
    assert_eq!(get(&index, &format!("/page/{f}/2")).status, 404);
}

#[test]
fn unknown_paths_ids_and_methods() {
    let t = TmpDir::new("srv-404");
    let (index, _) = setup(&t);
    for path in [
        "/book/9999",
        "/book/abc",
        "/book/",
        "/page/9999/0",
        "/page/1",
        "/page/1/0/x",
        "/nope",
        "/book/1/",
    ] {
        assert_eq!(get(&index, path).status, 404, "{path}");
    }
    let r = handle(&index, None, "POST / HTTP/1.1\r\n\r\n");
    assert_eq!((r.status, r.allow), (405, Some("GET")));
    assert_eq!(handle(&index, None, "HEAD / HTTP/1.1\r\n\r\n").status, 405);
    assert_eq!(handle(&index, None, "garbage\r\n\r\n").status, 400);
    assert_eq!(handle(&index, None, "").status, 400);
}

#[test]
fn root_filter_hides_other_books() {
    let t = TmpDir::new("srv-root");
    let (index, root) = setup(&t);
    let a = id_of(&index, "a");
    let other = t.0.join("elsewhere");
    let req = format!("GET /page/{a}/0 HTTP/1.1\r\n\r\n");
    assert_eq!(handle(&index, Some(&root), &req).status, 200);
    assert_eq!(handle(&index, Some(&other), &req).status, 404);
}

#[test]
fn title_is_html_escaped() {
    let t = TmpDir::new("srv-esc");
    let root = t.0.join("books");
    write_zip(&root.join("a&b.zip"), &["1.png"]);
    let mut index = Index::open(&t.0.join("db/index.db")).unwrap();
    index.reindex(&root).unwrap();
    let html = text(&get(&index, "/"));
    assert!(html.contains("a&amp;b"));
}

fn http_get(addr: std::net::SocketAddr, path: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\n\r\n").unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    out
}

#[test]
fn serves_over_real_socket() {
    let t = TmpDir::new("srv-sock");
    let (index, _) = setup(&t);
    let a = id_of(&index, "a");
    let db = index.path().to_path_buf();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || server::serve(listener, db, None));

    let raw = http_get(addr, &format!("/page/{a}/0"));
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    assert!(head.starts_with("HTTP/1.0 200 OK"), "{head}");
    assert!(head.contains("Content-Type: image/png"));
    assert!(head.contains(&format!("Content-Length: {}", png_bytes().len())));
    assert_eq!(&raw[split + 4..], png_bytes().as_slice());

    let missing = http_get(addr, &format!("/page/{a}/3"));
    assert!(missing.starts_with(b"HTTP/1.0 404 Not Found"));
}
