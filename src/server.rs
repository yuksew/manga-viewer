//! ローカル HTTP サーバ（p0-3）。`127.0.0.1` だけに bind し、索引の冊を画像で配る。
//!
//! * `GET /` — 冊の一覧（索引の自然順）
//! * `GET /book/<id>[?p=N]` — N 頁目（既定 0）を `<img>` で出す。prev/next と「N/M ページ」つき
//! * `GET /page/<id>/<n>` — 画像バイト（`decode` で実復号して検証済み。壊れていれば 422。Content-Type は magic から）
//! * `GET /image/<id>/<n>` — 同じ画像を `Content-Disposition: inline` つきで（ブラウザ直表示用）
//!
//! **デバッグ専用**（P0 の最終形は `view`）。view との差の扱いは docs/p0-6-design-options.md §6。
//!
//! 画像のパスは URL から組まず、索引の path と `Book` の頁一覧だけから決める。
//! 応答は `Connection: close`（HTTP/1.0 流儀）で、接続ごとに 1 スレッド。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::book::Book;
use crate::decode;
use crate::index::{BookRow, Index, Status};

/// リクエストヘッダの上限（これを超えたら読むのをやめる）。
const MAX_HEADER: usize = 8 * 1024;

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// 405 のときだけ付く `Allow`
    pub allow: Option<&'static str>,
    /// 追加ヘッダ（画像の `Content-Disposition` など）
    pub headers: Vec<(&'static str, String)>,
}

impl Response {
    fn html(status: u16, body: String) -> Response {
        Response {
            status,
            content_type: "text/html; charset=utf-8",
            body: body.into_bytes(),
            allow: None,
            headers: Vec::new(),
        }
    }

    fn error(status: u16, message: &str) -> Response {
        let body = page(
            "エラー",
            &format!(
                "<p>{} {}</p><p><a href=\"/\">一覧へ</a></p>",
                status,
                esc(message)
            ),
        );
        Response::html(status, body)
    }

    /// ステータス行・ヘッダ・本体を並べた送出バイト列。
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.0 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.status,
            reason(self.status),
            self.content_type,
            self.body.len()
        );
        if let Some(allow) = self.allow {
            head.push_str(&format!("Allow: {allow}\r\n"));
        }
        for (k, v) in &self.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        422 => "Unprocessable Entity",
        _ => "Internal Server Error",
    }
}

/// 拡張子（大小文字を問わない）から Content-Type を決める。未対応は `None`。
#[must_use]
pub fn content_type(name: &str) -> Option<&'static str> {
    let ext = Path::new(name)
        .extension()?
        .to_string_lossy()
        .to_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"ja\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{}</title></head><body>{}</body></html>",
        esc(title),
        body
    )
}

/// 数字だけの文字列を数にする（`+1` や空は不可）。
fn parse_num<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn not_found() -> Response {
    Response::error(404, "見つかりません")
}

/// リクエスト（請求行 + ヘッダの文字列）から応答を作る。I/O はしない（冊の読み出しを除く）。
///
/// `root` を渡すとその下の冊だけを扱う（無ければ索引の全部）。
pub fn handle(index: &Index, root: Option<&Path>, request: &str) -> Response {
    let line = request.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Response::error(400, "リクエスト行が不正です");
    };
    if !version.starts_with("HTTP/") {
        return Response::error(400, "リクエスト行が不正です");
    }
    if method != "GET" {
        let mut r = Response::error(405, "GET だけ使えます");
        r.allow = Some("GET");
        return r;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let segs: Vec<&str> = match path.strip_prefix('/') {
        Some(p) => p.split('/').collect(),
        None => return not_found(),
    };
    let result = match segs.as_slice() {
        [""] => list_page(index, root),
        ["book", id] => match parse_num::<i64>(id) {
            Some(id) => book_page(index, root, id, query),
            None => Ok(not_found()),
        },
        ["page", id, n] => match (parse_num::<i64>(id), parse_num::<usize>(n)) {
            (Some(id), Some(n)) => image(index, root, id, n, false),
            _ => Ok(not_found()),
        },
        ["image", id, n] => match (parse_num::<i64>(id), parse_num::<usize>(n)) {
            (Some(id), Some(n)) => image(index, root, id, n, true),
            _ => Ok(not_found()),
        },
        _ => Ok(not_found()),
    };
    result.unwrap_or_else(|msg| Response::error(500, &msg))
}

fn rows(index: &Index, root: Option<&Path>) -> Result<Vec<BookRow>, String> {
    match root {
        Some(root) => index.books_under(root),
        None => index.books(),
    }
    .map_err(|e| e.to_string())
}

fn list_page(index: &Index, root: Option<&Path>) -> Result<Response, String> {
    let mut body = String::from("<h1>books</h1><ul>");
    for row in rows(index, root)? {
        let note = if row.status == Status::Broken {
            " （壊れています）"
        } else {
            ""
        };
        body.push_str(&format!(
            "<li><a href=\"/book/{}\">{}</a> {} ページ{}</li>",
            row.id,
            esc(&row.title),
            row.page_count,
            note
        ));
    }
    body.push_str("</ul>");
    Ok(Response::html(200, page("books", &body)))
}

/// 索引から冊を引いて開く。無い・壊れている・開けない冊は `Err(応答)`。
fn open_book(
    index: &Index,
    root: Option<&Path>,
    id: i64,
) -> Result<Result<(BookRow, Book), Response>, String> {
    let Some(row) = rows(index, root)?.into_iter().find(|r| r.id == id) else {
        return Ok(Err(Response::error(404, "その冊は索引にありません")));
    };
    if row.status == Status::Broken {
        let why = row.error.clone().unwrap_or_default();
        return Ok(Err(Response::error(404, &format!("壊れた冊です: {why}"))));
    }
    match Book::open(&row.path, row.kind) {
        Ok(book) => Ok(Ok((row, book))),
        Err(e) => Ok(Err(Response::error(404, &format!("開けません: {e}")))),
    }
}

fn book_page(index: &Index, root: Option<&Path>, id: i64, query: &str) -> Result<Response, String> {
    let (row, book) = match open_book(index, root, id)? {
        Ok(found) => found,
        Err(resp) => return Ok(resp),
    };
    let total = book.page_count();
    let p = match query.split('&').find_map(|kv| kv.strip_prefix("p=")) {
        None => 0,
        Some(v) => match parse_num::<usize>(v) {
            Some(p) => p,
            None => return Ok(Response::error(404, "頁番号が不正です")),
        },
    };
    if p >= total {
        return Ok(Response::error(404, "頁が範囲外です"));
    }
    let img = if content_type(book.page_names()[p]).is_some() {
        format!(
            "<img src=\"/page/{id}/{p}\" style=\"max-width:100%\" alt=\"{}\">",
            p + 1
        )
    } else {
        "<p>この形式の頁は未対応です</p>".to_string()
    };
    let mut nav = String::new();
    if p > 0 {
        nav.push_str(&format!("<a href=\"/book/{id}?p={}\">prev</a> ", p - 1));
    }
    nav.push_str(&format!("{}/{} ページ", p + 1, total));
    if p + 1 < total {
        nav.push_str(&format!(" <a href=\"/book/{id}?p={}\">next</a>", p + 1));
    }
    let body = format!(
        "<h1>{}</h1><p><a href=\"/\">一覧</a></p><p>{nav}</p>{img}",
        esc(&row.title)
    );
    Ok(Response::html(200, page(&row.title, &body)))
}

fn image(
    index: &Index,
    root: Option<&Path>,
    id: i64,
    n: usize,
    inline: bool,
) -> Result<Response, String> {
    let (_, book) = match open_book(index, root, id)? {
        Ok(found) => found,
        Err(resp) => return Ok(resp),
    };
    let Some(name) = book.page_names().get(n).map(|s| (*s).to_string()) else {
        return Ok(Response::error(404, "頁が範囲外です"));
    };
    if content_type(&name).is_none() {
        return Ok(Response::error(404, "この形式の頁は未対応です"));
    }
    let body = match book.read_page_bytes(n) {
        Ok(body) => body,
        Err(e) => return Ok(Response::error(404, &format!("頁を読めません: {e}"))),
    };
    // view と同じ検証（imgcheck + 実復号）を通し、文言も揃える。壊れていれば 422。
    let kind = match decode::decode_rgba(&body) {
        Ok(decoded) => decoded.kind,
        Err(e) => {
            return Ok(Response::error(
                422,
                &format!("この頁は表示できません: {e}"),
            ));
        }
    };
    let mut headers = vec![("X-Content-Type-Options", "nosniff".to_string())];
    if inline {
        headers.push((
            "Content-Disposition",
            format!("inline; filename=\"{}.{}\"", n + 1, kind.ext()),
        ));
    }
    Ok(Response {
        status: 200,
        content_type: kind.content_type(),
        body,
        allow: None,
        headers,
    })
}

/// 接続 1 本を処理する（ヘッダを読み、応答を書いて閉じる）。
fn serve_connection(mut stream: TcpStream, db: &Path, root: Option<&Path>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < MAX_HEADER {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let request = String::from_utf8_lossy(&buf);
    let response = match Index::open(db) {
        Ok(index) => handle(&index, root, &request),
        Err(e) => Response::error(500, &e.to_string()),
    };
    let _ = stream.write_all(&response.to_bytes());
    let _ = stream.flush();
}

/// `listener` で待ち受けて配り続ける（戻らない）。
pub fn serve(listener: TcpListener, db: PathBuf, root: Option<PathBuf>) {
    for stream in listener.incoming().flatten() {
        let db = db.clone();
        let root = root.clone();
        std::thread::spawn(move || serve_connection(stream, &db, root.as_deref()));
    }
}
