//! 索引層（`Index::open` / `reindex` / `books` / `books_under`）の試験。
//! 監査（p0-2a / p0-2b）で変異が全部生き残った穴を塞ぐ。検査データは合成のみ。

mod common;

use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{DenyList, TmpDir, write_folder, write_zip};
use manga_viewer::index::{Index, SCHEMA_VERSION, Status};

fn open(t: &TmpDir) -> Index {
    Index::open(&t.0.join("db/index.db")).unwrap()
}

fn titles(index: &Index) -> Vec<String> {
    index
        .books()
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect()
}

/// ファイルの mtime だけを進める（中身とサイズは同じ）。
fn bump_mtime(path: &Path) {
    let file = File::options().write(true).open(path).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    file.set_modified(later).unwrap();
}

/// ディレクトリの mtime を戻す（Windows はバックアップ意味論つきで開く）。
fn set_dir_mtime(dir: &Path, time: SystemTime) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let handle = File::options()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)
        .unwrap();
    handle.set_modified(time).unwrap();
}

fn root_with_books(t: &TmpDir, name: &str) -> PathBuf {
    let root = t.0.join(name);
    write_zip(&root.join("a.zip"), &["1.png", "2.png"]);
    write_zip(&root.join("b.zip"), &["1.png"]);
    write_folder(&root.join("f"), &["1.png", "2.png", "3.png"]);
    root
}

fn cover_count(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT count(*) FROM covers", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn second_index_rescans_nothing() {
    let t = TmpDir::new("idx-unchanged");
    let root = root_with_books(&t, "root");
    let mut index = open(&t);

    let first = index.reindex(&root).unwrap();
    assert_eq!(
        (first.discovered, first.added, first.rescanned()),
        (3, 3, 3)
    );
    assert_eq!(index.count().unwrap(), 3);
    let pages: i64 = index.books().unwrap().iter().map(|r| r.page_count).sum();
    assert_eq!(pages, 6);

    let second = index.reindex(&root).unwrap();
    assert_eq!(second.unchanged, 3);
    assert_eq!(second.rescanned(), 0);
    assert_eq!(second.removed, 0);
    assert_eq!(index.count().unwrap(), 3);
}

#[test]
fn mtime_change_alone_reindexes_that_book() {
    let t = TmpDir::new("idx-mtime");
    let root = root_with_books(&t, "root");
    let mut index = open(&t);
    index.reindex(&root).unwrap();

    bump_mtime(&root.join("a.zip"));
    let report = index.reindex(&root).unwrap();
    assert_eq!((report.updated, report.unchanged), (1, 2));
}

#[test]
fn size_change_alone_reindexes_that_book() {
    let t = TmpDir::new("idx-size");
    let root = root_with_books(&t, "root");
    let mut index = open(&t);
    index.reindex(&root).unwrap();

    // 頁を足したあとフォルダの mtime を元へ戻し、size の違いだけを残す
    let folder = root.join("f");
    let before = fs::metadata(&folder).unwrap().modified().unwrap();
    fs::write(folder.join("4.png"), common::png_bytes()).unwrap();
    set_dir_mtime(&folder, before);
    let report = index.reindex(&root).unwrap();
    assert_eq!(report.updated, 1);
    let row = index
        .books()
        .unwrap()
        .into_iter()
        .find(|r| r.title == "f")
        .unwrap();
    assert_eq!(row.page_count, 4);
}

#[test]
fn deleted_books_are_pruned() {
    let t = TmpDir::new("idx-removed");
    let root = root_with_books(&t, "root");
    let mut index = open(&t);
    index.reindex(&root).unwrap();

    fs::remove_file(root.join("a.zip")).unwrap();
    fs::remove_dir_all(root.join("f")).unwrap();
    let report = index.reindex(&root).unwrap();
    assert_eq!(report.removed, 2);
    assert_eq!(report.discovered, 1);
    assert_eq!(titles(&index), ["b"]);
    assert_eq!(index.count().unwrap(), 1);
}

#[test]
fn pruning_never_touches_other_roots() {
    let t = TmpDir::new("idx-scope");
    let one = root_with_books(&t, "one");
    // 接頭辞だけ同じ隣のフォルダも巻き込まない
    let one_extra = t.0.join("one-extra");
    write_zip(&one_extra.join("x.zip"), &["1.png"]);
    let mut index = open(&t);
    index.reindex(&one).unwrap();
    index.reindex(&one_extra).unwrap();
    assert_eq!(index.count().unwrap(), 4);

    fs::remove_file(one.join("a.zip")).unwrap();
    let report = index.reindex(&one).unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(index.count().unwrap(), 3);
    assert!(titles(&index).contains(&"x".to_string()));
}

#[test]
fn limit_does_not_prune() {
    let t = TmpDir::new("idx-limit");
    let root = root_with_books(&t, "root");
    let mut index = open(&t);
    index.reindex(&root).unwrap();
    let report = index.reindex_upto(&root, Some(1)).unwrap();
    assert_eq!(report.removed, 0);
    assert_eq!(index.count().unwrap(), 3);
}

#[test]
fn list_is_in_natural_order_not_insertion_order() {
    let t = TmpDir::new("idx-order");
    let root = t.0.join("root");
    write_zip(&root.join("book10.zip"), &["1.png"]);
    write_zip(&root.join("book1.zip"), &["1.png"]);
    let mut index = open(&t);
    index.reindex(&root).unwrap();
    // 後から入った book2 は id が大きいが、一覧では book1 と book10 の間に来る
    write_zip(&root.join("book2.zip"), &["1.png"]);
    index.reindex(&root).unwrap();
    assert_eq!(titles(&index), ["book1", "book2", "book10"]);
}

#[test]
fn books_under_returns_only_that_root() {
    let t = TmpDir::new("idx-under");
    let one = root_with_books(&t, "one");
    let two = t.0.join("one-extra");
    write_zip(&two.join("x.zip"), &["1.png"]);
    let mut index = open(&t);
    index.reindex(&one).unwrap();
    index.reindex(&two).unwrap();

    assert_eq!(index.books_under(&one).unwrap().len(), 3);
    let under_two = index.books_under(&two).unwrap();
    assert_eq!(under_two.len(), 1);
    assert_eq!(under_two[0].title, "x");
    assert_eq!(index.books().unwrap().len(), 4);
}

#[test]
fn broken_book_is_kept_with_reason_and_stays_unchanged() {
    let t = TmpDir::new("idx-broken");
    let root = t.0.join("root");
    write_zip(&root.join("empty.zip"), &[]);
    write_zip(&root.join("ok.zip"), &["1.png"]);
    let mut index = open(&t);
    let first = index.reindex(&root).unwrap();
    assert_eq!(first.broken, 1);
    let row = index
        .books()
        .unwrap()
        .into_iter()
        .find(|r| r.title == "empty")
        .unwrap();
    assert_eq!(row.status, Status::Broken);
    assert!(row.error.is_some());

    let second = index.reindex(&root).unwrap();
    assert_eq!((second.rescanned(), second.broken), (0, 1));
}

#[test]
fn updating_a_book_drops_its_stale_cover() {
    let t = TmpDir::new("idx-cover");
    let root = root_with_books(&t, "root");
    let db = t.0.join("db/index.db");
    let mut index = Index::open(&db).unwrap();
    index.reindex(&root).unwrap();
    let id_of_a = index
        .books()
        .unwrap()
        .into_iter()
        .find(|r| r.title == "a")
        .unwrap()
        .id;
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO covers (book_id, w, h, jpeg) VALUES (?1, 1, 1, x'00')",
            [id_of_a],
        )
        .unwrap();
    }
    assert_eq!(cover_count(&db), 1);

    // 変わっていない冊の表紙は残る
    index.reindex(&root).unwrap();
    assert_eq!(cover_count(&db), 1);
    // 開き直した冊の表紙は消える
    bump_mtime(&root.join("a.zip"));
    index.reindex(&root).unwrap();
    assert_eq!(cover_count(&db), 0);
}

#[test]
fn schema_version_mismatch_rebuilds_the_db() {
    let t = TmpDir::new("idx-version");
    let root = root_with_books(&t, "root");
    let db = t.0.join("db/index.db");
    {
        let mut index = Index::open(&db).unwrap();
        index.reindex(&root).unwrap();
        assert_eq!(index.user_version().unwrap(), SCHEMA_VERSION);
    }
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA user_version = 999;").unwrap();
    }
    let index = Index::open(&db).unwrap();
    assert_eq!(index.user_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(index.count().unwrap(), 0);
}

#[test]
fn matching_schema_version_keeps_rows() {
    let t = TmpDir::new("idx-keep");
    let root = root_with_books(&t, "root");
    let db = t.0.join("db/index.db");
    {
        let mut index = Index::open(&db).unwrap();
        index.reindex(&root).unwrap();
    }
    assert_eq!(Index::open(&db).unwrap().count().unwrap(), 3);
}

/// 監査再現: ルートの綴り（大文字小文字）を変えて指定しても行が増えない。
#[cfg(windows)]
#[test]
fn root_spelled_with_different_case_does_not_duplicate_rows() {
    let t = TmpDir::new("idx-case");
    let root = root_with_books(&t, "root");
    let mut index = open(&t);
    index.reindex(&root).unwrap();
    assert_eq!(index.count().unwrap(), 3);

    // 更新が要る冊を作ってから、別の綴りで索引する
    write_zip(&root.join("a.zip"), &["1.png", "2.png", "3.png"]);
    let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
    let report = index.reindex(&upper).unwrap();
    assert_eq!(report.discovered, 3);
    assert_eq!(report.updated, 1);
    assert_eq!(index.count().unwrap(), 3);
    let pages: i64 = index.books().unwrap().iter().map(|r| r.page_count).sum();
    assert_eq!(pages, 7);
    assert_eq!(index.books_under(&root).unwrap().len(), 3);
}

#[test]
fn non_sqlite_file_is_reported_without_touching_it() {
    let t = TmpDir::new("idx-corrupt");
    let db = t.0.join("index.db");
    fs::write(&db, b"this is not a database, just text ".repeat(40)).unwrap();
    let before = fs::read(&db).unwrap();

    let err = Index::open(&db).err().expect("開けないはず");
    let text = err.to_string();
    assert!(text.contains("壊れています"), "{text}");
    assert!(text.contains("--db"), "{text}");
    assert_eq!(
        fs::read(&db).unwrap(),
        before,
        "利用者のファイルは書き換えない"
    );
}

fn run_cli(args: &[&OsStr]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_manga-viewer"))
        .args(args)
        .env_remove("MANGA_VIEWER_DATA")
        .output()
        .unwrap()
}

#[test]
fn cli_reports_corrupt_db_with_guidance_and_exit_1() {
    let t = TmpDir::new("cli-corrupt");
    let db = t.0.join("index.db");
    fs::write(&db, b"garbage garbage garbage ".repeat(50)).unwrap();
    let root = root_with_books(&t, "root");
    for sub in ["index", "list"] {
        let mut args: Vec<&OsStr> = vec![sub.as_ref()];
        if sub == "index" {
            args.push(root.as_os_str());
        }
        args.push("--db".as_ref());
        args.push(db.as_os_str());
        let out = run_cli(&args);
        assert_eq!(out.status.code(), Some(1), "{sub}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("壊れています"), "{sub}: {err}");
        assert!(!err.contains("file is not a database"), "{sub}: {err}");
    }
}

#[test]
fn cli_list_shows_indexed_books_in_order() {
    let t = TmpDir::new("cli-list");
    let root = root_with_books(&t, "root");
    let db = t.0.join("x/index.db");
    let out = run_cli(&[
        "index".as_ref(),
        root.as_os_str(),
        "--db".as_ref(),
        db.as_os_str(),
    ]);
    assert!(out.status.success());
    let out = run_cli(&["list".as_ref(), "--db".as_ref(), db.as_os_str()]);
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    let got: Vec<&str> = s.lines().map(|l| l.split('\t').nth(1).unwrap()).collect();
    assert_eq!(got, ["a", "b", "f"]);
}

/// #5: 再索引の時に読めなかった子ディレクトリの下の冊は「消えた」扱いにしない。
#[test]
fn books_under_unreadable_subdir_are_kept_and_reported() {
    let t = TmpDir::new("idx-deny");
    let root = t.0.join("root");
    write_zip(&root.join("ok/vol1.zip"), &["1.png"]);
    write_zip(&root.join("locked/vol2.zip"), &["1.png"]);
    let mut index = open(&t);
    let first = index.reindex(&root).unwrap();
    assert_eq!(first.discovered, 2);
    assert!(first.skipped.is_empty());

    let deny = DenyList::new(&root.join("locked"));
    let report = index.reindex(&root).unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].path, root.join("locked"));
    assert_eq!(report.removed, 0);
    assert_eq!(titles(&index), ["vol1", "vol2"]);

    // 読めるように戻した後、本当に消えた冊は従来どおり消える
    drop(deny);
    fs::remove_file(root.join("locked/vol2.zip")).unwrap();
    let report = index.reindex(&root).unwrap();
    assert!(report.skipped.is_empty());
    assert_eq!(report.removed, 1);
    assert_eq!(titles(&index), ["vol1"]);
}
