//! 索引の永続化（設計 §2.2）。
//!
//! SQLite（WAL）へ冊を 1 行ずつ入れる。頁は 1 行ずつ入らず `books.page_count` に入る
//! （設計 §2.2 のスキーマがその形。作り直せる物だけ入れる方針なので、DB が無くても
//! 走査し直せば同じ物ができる）。
//!
//! 突き合わせの鍵は `path`（監査所見 3: 題名は一意でない）。`mtime_ns` と `size` が
//! 同じ冊は中身を開き直さない（設計 §2.2 の差分索引）。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use rusqlite::{Connection, Row};
use thiserror::Error;

use crate::book::Book;
use crate::natural::natural_cmp;
use crate::scan::{self, BookEntry, BookKind};

/// `PRAGMA user_version` の値。合わなければ DB を捨てて作り直す（設計 §2.2。移行は作らない）。
pub const SCHEMA_VERSION: i64 = 2;

/// 1 回のトランザクションに入れる冊数（設計 §2.2）。
pub const BATCH_SIZE: usize = 50;

/// 索引 DB の置き場所を差し替える環境変数（設計 §2.2。試験用）。
pub const DATA_ENV: &str = "MANGA_VIEWER_DATA";

/// `MANGA_VIEWER_DATA` が無いときの既定フォルダ名（`%LOCALAPPDATA%` の下）。
pub const DATA_DIR_NAME: &str = "manga-viewer";

/// 索引 DB のファイル名。
pub const DB_FILE_NAME: &str = "index.db";

/// p0-2c が埋める `covers` も先に作っておく（p0-1b の段では空。設計 §2.2）。
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS books (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    mtime_ns INTEGER NOT NULL,
    size INTEGER NOT NULL,
    title TEXT NOT NULL,
    page_count INTEGER NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    scanned_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS covers (
    book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    w INTEGER,
    h INTEGER,
    jpeg BLOB
);
";

const SELECT_BOOKS: &str = "SELECT id, path, kind, title, page_count, status, error, mtime_ns, size, scanned_at FROM books";

const UPSERT_BOOK: &str =
    "INSERT INTO books (key, path, kind, mtime_ns, size, title, page_count, status, error, scanned_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(key) DO UPDATE SET
    path = excluded.path,
    kind = excluded.kind,
    mtime_ns = excluded.mtime_ns,
    size = excluded.size,
    title = excluded.title,
    page_count = excluded.page_count,
    status = excluded.status,
    error = excluded.error,
    scanned_at = excluded.scanned_at";

const DELETE_COVER: &str =
    "DELETE FROM covers WHERE book_id = (SELECT id FROM books WHERE key = ?1)";

const DELETE_BOOK: &str = "DELETE FROM books WHERE id = ?1";

/// 索引 DB の失敗。
#[derive(Error, Debug)]
pub enum IndexError {
    #[error(
        "索引 DB が壊れています（SQLite のファイルではありません）: {}\n削除するか、`--db` で別のファイルを指定してください",
        .path.display()
    )]
    NotADatabase { path: PathBuf },
    #[error("索引 DB のフォルダを作れません: {}", .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("索引 DB を開けません: {}", .path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("索引 DB の読み書きに失敗しました（{sql}）: {source}")]
    Sql {
        sql: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("HOME が取れません。`--db` か環境変数 {DATA_ENV} を使ってください")]
    NoHome,
    #[error(transparent)]
    Scan(#[from] std::io::Error),
}

/// 冊の状態（設計 A-6: 壊れた冊は一覧に出る。理由は `error` に入る）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// 開ける。
    Ok,
    /// 一覧には出るが開けない。
    Broken,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Broken => "broken",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "ok" => Some(Status::Ok),
            "broken" => Some(Status::Broken),
            _ => None,
        }
    }
}

fn kind_str(kind: BookKind) -> &'static str {
    match kind {
        BookKind::Zip => "zip",
        BookKind::Folder => "folder",
    }
}

fn kind_parse(s: &str) -> Option<BookKind> {
    match s {
        "zip" => Some(BookKind::Zip),
        "folder" => Some(BookKind::Folder),
        _ => None,
    }
}

/// DB に入った 1 冊（`list` と GUI の一覧が読む）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookRow {
    /// 自明な行番号。
    pub id: i64,
    /// 冊の path（DB に入れた文字列そのまま。開くときはこれを使う）。
    pub path: PathBuf,
    /// ZIP かフォルダか。
    pub kind: BookKind,
    /// 表示タイトル。
    pub title: String,
    /// 頁数（設計 §2.2。頁そのものは DB に入れない）。
    pub page_count: i64,
    /// ok / broken。
    pub status: Status,
    /// broken の理由（設計 A-6）。
    pub error: Option<String>,
    /// 差分索引の鍵（更新時刻 ns）。
    pub mtime_ns: i64,
    /// 差分索引の鍵（サイズ）。
    pub size: i64,
    /// 索引した時刻（epoch 秒）。
    pub scanned_at: i64,
}

/// 1 回の索引がやったこと（`index` が件数と所要を出す。設計 §3.3）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// 走査で見つけた冊数。
    pub discovered: usize,
    /// DB に無くて追加した冊数。
    pub added: usize,
    /// 鍵が変わって開き直した冊数。
    pub updated: usize,
    /// 鍵が変わらず開き直さなかった冊数。
    pub unchanged: usize,
    /// 消えていたので DB から消した冊数。
    pub removed: usize,
    /// 索引後に broken だった冊数。
    pub broken: usize,
    /// 走査から書き込み完了までの実時間。
    pub elapsed: Duration,
}

impl Report {
    /// 中身を開き直した冊数。差分索引が効いていれば 0（設計 §5 の合格条件）。
    #[must_use]
    pub fn rescanned(&self) -> usize {
        self.added + self.updated
    }
}

/// 索引 DB。
pub struct Index {
    conn: Connection,
    path: PathBuf,
}

impl Index {
    /// 索引 DB を開いて準備する（ファイルもスキーマも無ければ作る）。
    pub fn open(path: &Path) -> Result<Index, IndexError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|source| IndexError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path).map_err(|source| IndexError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        prepare(&conn).map_err(|err| match err {
            IndexError::Sql { source, .. } if is_not_a_database(&source) => {
                IndexError::NotADatabase {
                    path: path.to_path_buf(),
                }
            }
            other => other,
        })?;
        Ok(Index {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// 既定の場所（設計 §2.2: `MANGA_VIEWER_DATA` があればそこ）から開く。
    pub fn open_default() -> Result<Index, IndexError> {
        Index::open(&default_db_path()?)
    }

    /// 開いている DB ファイルのパス。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// DB に入った冊の数。
    pub fn count(&self) -> Result<i64, IndexError> {
        sql(
            "count",
            self.conn
                .query_row("SELECT count(*) FROM books", [], |row| row.get(0)),
        )
    }

    /// `PRAGMA user_version` の値（試験と診断用）。
    pub fn user_version(&self) -> Result<i64, IndexError> {
        sql(
            "user_version",
            self.conn
                .query_row("PRAGMA user_version", [], |row| row.get(0)),
        )
    }

    /// 一覧。題名の自然順（同じ題名は path の順）で、p0-1a の走査順と同じ。
    pub fn books(&self) -> Result<Vec<BookRow>, IndexError> {
        let mut stmt = sql("list", self.conn.prepare(SELECT_BOOKS))?;
        let mut rows = sql("list", stmt.query([]))?;
        let mut out = Vec::new();
        while let Some(row) = sql("list", rows.next())? {
            out.push(sql("list", row_to_book(row))?);
        }
        out.sort_by(|a, b| natural_cmp(&a.title, &b.title).then_with(|| a.path.cmp(&b.path)));
        Ok(out)
    }

    /// `root` の下に入った冊だけ（一覧は 1 ルートで出す。設計 A-2）。
    pub fn books_under(&self, root: &Path) -> Result<Vec<BookRow>, IndexError> {
        let root_key = path_key(&absolute(root)?);
        Ok(self
            .books()?
            .into_iter()
            .filter(|row| under_root(&path_key(&row.path), &root_key))
            .collect())
    }

    /// root を走査して DB に反映する（設計 §2.2 の突き合わせ: 追加・更新・削除）。
    pub fn reindex(&mut self, root: &Path) -> Result<Report, IndexError> {
        self.reindex_upto(root, None)
    }

    /// 自然順の先頭 `limit` 冊だけを開いて書く（B-3(a) の測定用）。
    /// `limit` 付きのときは削除の整理をしない（まだ見ていない冊を消さないため）。
    /// 一覧を順に膨らませる作りは p0-3a。
    pub fn reindex_upto(
        &mut self,
        root: &Path,
        limit: Option<usize>,
    ) -> Result<Report, IndexError> {
        let started = Instant::now();
        let root = absolute(root)?;
        let mut found = scan::scan_root(&root)?;
        if let Some(n) = limit {
            found.truncate(n);
        }
        let existing = self.existing_rows()?;

        let mut report = Report {
            discovered: found.len(),
            ..Report::default()
        };
        let mut fresh: Vec<&BookEntry> = Vec::new();
        let mut alive: HashSet<String> = HashSet::with_capacity(found.len());
        for entry in &found {
            let key = path_key(&entry.path);
            alive.insert(key.clone());
            let size = entry_size(entry);
            match existing.get(&key) {
                // 鍵が同じ冊は中身を開き直さない（設計 §2.2）
                Some(row) if row.mtime_ns == entry.mtime_ns && row.size == size => {
                    report.unchanged += 1;
                    if row.status == Status::Broken {
                        report.broken += 1;
                    }
                }
                Some(_) => {
                    report.updated += 1;
                    fresh.push(entry);
                }
                None => {
                    report.added += 1;
                    fresh.push(entry);
                }
            }
        }

        // 構造索引は冊ごとに独立なので rayon、DB 書き込みは単一スレッド（設計 §2.2）。
        let now = now_secs();
        for chunk in fresh.chunks(BATCH_SIZE) {
            let opened: Vec<(i64, Status, Option<String>)> = chunk
                .into_par_iter()
                .map(|entry| match Book::open(&entry.path, entry.kind) {
                    Ok(book) => (
                        i64::try_from(book.page_count()).unwrap_or(i64::MAX),
                        Status::Ok,
                        None,
                    ),
                    Err(err) => (0, Status::Broken, Some(err.to_string())),
                })
                .collect();

            let tx = sql("begin", self.conn.transaction())?;
            {
                let mut stmt = sql("upsert", tx.prepare(UPSERT_BOOK))?;
                for (entry, (page_count, status, error)) in chunk.iter().zip(&opened) {
                    sql(
                        "upsert",
                        stmt.execute(rusqlite::params![
                            path_key(&entry.path),
                            entry.path.to_string_lossy(),
                            kind_str(entry.kind),
                            entry.mtime_ns,
                            entry_size(entry),
                            entry.title,
                            page_count,
                            status.as_str(),
                            error,
                            now
                        ]),
                    )?;
                    // 開き直した冊の表紙は古い（`covers` は p0-2c が埋める。設計 §2.2）
                    sql(
                        "cover",
                        tx.execute(DELETE_COVER, rusqlite::params![path_key(&entry.path)]),
                    )?;
                }
            }
            sql("commit", tx.commit())?;
            report.broken += opened
                .iter()
                .filter(|(_, s, _)| *s == Status::Broken)
                .count();
        }

        // 消えた冊を消す。走査した root の下だけ（別の root が入った DB を巻かない）。
        // `limit` 付きのときはまだ見ていない冊があるので削除しない。
        if limit.is_none() {
            let root_key = path_key(&root);
            let tx = sql("begin", self.conn.transaction())?;
            for (key, row) in &existing {
                if alive.contains(key) || !under_root(key, &root_key) {
                    continue;
                }
                sql("delete", tx.execute(DELETE_BOOK, rusqlite::params![row.id]))?;
                report.removed += 1;
            }
            sql("commit", tx.commit())?;
        }

        report.elapsed = started.elapsed();
        Ok(report)
    }

    /// 既に入っている冊を `path` の鍵で引けるようにして返す。
    fn existing_rows(&self) -> Result<HashMap<String, BookRow>, IndexError> {
        let books = self.books()?;
        let mut map = HashMap::with_capacity(books.len());
        for row in books {
            map.insert(path_key(&row.path), row);
        }
        Ok(map)
    }
}

/// SQLite のファイルではない（中身が別物）ときの失敗か。
fn is_not_a_database(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::NotADatabase
    )
}

/// スキーマを準備する。`user_version` が合わなければ捨てて作り直す（設計 §2.2）。
fn prepare(conn: &Connection) -> Result<(), IndexError> {
    // WAL（設計 §2.2）。`covers` の ON DELETE CASCADE には接続ごとに foreign_keys が必要。
    sql(
        "pragmas",
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA foreign_keys=ON;",
        ),
    )?;
    let version: i64 = sql(
        "user_version",
        conn.query_row("PRAGMA user_version", [], |row| row.get(0)),
    )?;
    if version != SCHEMA_VERSION {
        sql(
            "rebuild",
            conn.execute_batch(
                "DROP TABLE IF EXISTS covers;
DROP TABLE IF EXISTS books;",
            ),
        )?;
        sql("schema", conn.execute_batch(SCHEMA))?;
        let set = format!("PRAGMA user_version = {SCHEMA_VERSION};");
        sql("user_version", conn.execute_batch(&set))?;
    } else {
        sql("schema", conn.execute_batch(SCHEMA))?;
    }
    Ok(())
}

fn row_to_book(row: &Row<'_>) -> rusqlite::Result<BookRow> {
    let kind: String = row.get(2)?;
    let status: String = row.get(5)?;
    Ok(BookRow {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        // 書くのは自分の値だけ（`user_version` が合わなければ作り直す）。読めない値は安全側へ倒す
        kind: kind_parse(&kind).unwrap_or(BookKind::Zip),
        title: row.get(3)?,
        page_count: row.get(4)?,
        status: Status::parse(&status).unwrap_or(Status::Broken),
        error: row.get(6)?,
        mtime_ns: row.get(7)?,
        size: row.get(8)?,
        scanned_at: row.get(9)?,
    })
}

/// 索引 DB の既定パス（設計 §2.2）。`MANGA_VIEWER_DATA` があればそれを優先する。
pub fn default_db_path() -> Result<PathBuf, IndexError> {
    db_path_for(std::env::var_os(DATA_ENV).as_deref())
}

/// `MANGA_VIEWER_DATA` の値（試験用）から既定パスを決める。
fn db_path_for(override_dir: Option<&std::ffi::OsStr>) -> Result<PathBuf, IndexError> {
    if let Some(dir) = override_dir.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(dir).join(DB_FILE_NAME));
    }
    let base = dirs::data_local_dir().ok_or(IndexError::NoHome)?;
    Ok(base.join(DATA_DIR_NAME).join(DB_FILE_NAME))
}

/// rusqlite の失敗に、どの SQL だったかを添える。
fn sql<T>(label: &'static str, result: rusqlite::Result<T>) -> Result<T, IndexError> {
    result.map_err(|source| IndexError::Sql { sql: label, source })
}

/// 差分索引の鍵に使う path の形。Windows は大文字小文字を区別しないので、
/// 区切りを `/` に揃えて小文字化しておく。DB の `key` 列（UNIQUE）にこの形を入れ、
/// `path` 列には変換前の綴りを入れる（綴り違いのルートでも行が重ならない）。
fn path_key(path: &Path) -> String {
    let slashes = path.to_string_lossy().replace('\\', "/");
    let trimmed = slashes.trim_end_matches('/').to_lowercase();
    if trimmed.is_empty() {
        slashes.to_lowercase()
    } else {
        trimmed
    }
}

/// `key` が `root` の下にあるか（削除の範囲を走査した root に限る）。
fn under_root(key: &str, root: &str) -> bool {
    key == root || (key.starts_with(root) && key[root.len()..].starts_with('/'))
}

/// 入力を絶対パスに（DB には絶対パスを入れる。相対 `--root` でも鍵がブレない）。
fn absolute(root: &Path) -> Result<PathBuf, IndexError> {
    let abs = std::path::absolute(root)?;
    let text = abs.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        // `\\?\C:\...` になると DB に残る表記が揃わない
        Some(stripped) => Ok(PathBuf::from(stripped)),
        None => Ok(abs),
    }
}

/// `BookEntry.size`（`u64`）を DB の INTEGER へ。
fn entry_size(entry: &BookEntry) -> i64 {
    i64::try_from(entry.size).unwrap_or(i64::MAX)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_key_normalizes_separators_and_case() {
        assert_eq!(path_key(Path::new(r"C:\Books\A.zip")), "c:/books/a.zip");
        assert_eq!(
            path_key(Path::new("c:/books/a.zip")),
            path_key(Path::new(r"C:\BOOKS\A.ZIP"))
        );
        // 末尾の区切りは鍵から落とす（root と子を比べる）
        assert_eq!(
            path_key(Path::new(r"C:\Books\")),
            path_key(Path::new(r"C:\Books"))
        );
        assert_eq!(path_key(Path::new(r"C:\")), "c:");
    }

    #[test]
    fn under_root_covers_children_but_not_neighbours() {
        let root = path_key(Path::new(r"C:\Books"));
        assert!(under_root(&root, &root));
        assert!(under_root(&path_key(Path::new(r"C:\Books\a.zip")), &root));
        assert!(under_root(&path_key(Path::new(r"C:\Books\a\b.zip")), &root));
        assert!(!under_root(
            &path_key(Path::new(r"C:\BooksExtra\a.zip")),
            &root
        ));
        assert!(!under_root(&path_key(Path::new(r"D:\Books\a.zip")), &root));
        // 別の root に入った冊は消さない
        let other = path_key(Path::new(r"C:\Other\a.zip"));
        assert!(!under_root(&other, &root));
    }

    #[test]
    fn db_path_prefers_the_env_override() {
        let over = db_path_for(Some(std::ffi::OsStr::new("Z:\\data"))).unwrap();
        assert_eq!(over, PathBuf::from("Z:\\data").join(DB_FILE_NAME));
        // 空文字は指定なしと同じ（既定は dirs なので、ファイル名だけ確認する）
        let none = db_path_for(None).unwrap();
        assert_eq!(
            none.file_name().and_then(|s| s.to_str()),
            Some(DB_FILE_NAME)
        );
        let empty = db_path_for(Some(std::ffi::OsStr::new(""))).unwrap();
        assert_eq!(empty, none);
    }

    #[test]
    fn absolute_strips_the_verbatim_prefix() {
        let got = absolute(Path::new(r"\\?\C:\Books\a.zip")).unwrap();
        assert_eq!(got, PathBuf::from(r"C:\Books\a.zip"));
        let rel = absolute(Path::new("a.zip")).unwrap();
        assert!(rel.is_absolute(), "{rel:?}");
    }

    #[test]
    fn status_and_kind_round_trip() {
        for status in [Status::Ok, Status::Broken] {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        assert_eq!(Status::parse("ok-ish"), None);
        for kind in [BookKind::Zip, BookKind::Folder] {
            assert_eq!(kind_parse(kind_str(kind)), Some(kind));
        }
        assert_eq!(kind_parse("rar"), None);
    }

    #[test]
    fn rescanned_counts_added_and_updated() {
        let report = Report {
            added: 3,
            updated: 1,
            unchanged: 8,
            ..Report::default()
        };
        assert_eq!(report.rescanned(), 4);
    }
}
