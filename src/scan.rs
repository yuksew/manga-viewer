use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::book::{is_image_name, is_zip_name};
use crate::natural::natural_cmp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookKind {
    Zip,
    Folder,
}

impl BookKind {
    /// DB の `kind` に書く文字列（設計 §2.2）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BookKind::Zip => "zip",
            BookKind::Folder => "folder",
        }
    }

    /// DB から読み戻す。知りでない値は `None`（呼び手は壊れた行として扱う）。
    #[must_use]
    pub fn parse(s: &str) -> Option<BookKind> {
        match s {
            "zip" => Some(BookKind::Zip),
            "folder" => Some(BookKind::Folder),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookEntry {
    pub path: PathBuf,
    pub kind: BookKind,
    pub mtime_ns: i64,
    pub size: u64,
    pub title: String,
}

/// 走査で読めずに飛ばした子ディレクトリ（権限エラー等）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    /// 読めなかった理由（`io::Error` の文言）。
    pub error: String,
}

/// 1 回の走査の結果。
#[derive(Debug, Clone, Default)]
pub struct Scan {
    /// 見つけた冊。題名の自然順。
    pub books: Vec<BookEntry>,
    /// 読めずに飛ばした子ディレクトリ。走査はその先を見ていない。
    pub skipped: Vec<Skipped>,
}

/// ルート直下（深さ1）とその1階層下（深さ2）を走査して冊を集める。結果は題名の自然順。
/// 中身は開かない（壊れた冊も一覧には出る。開けるかは `Book::open` で分かる）。
pub fn scan_root(root: &Path) -> std::io::Result<Vec<BookEntry>> {
    Ok(scan_root_full(root)?.books)
}

/// `scan_root` に加えて、読めずに飛ばした子ディレクトリも返す。root 自体が読めなければ Err。
pub fn scan_root_full(root: &Path) -> std::io::Result<Scan> {
    let mut scan = Scan::default();
    scan_dir(root, 0, &mut scan)?;
    scan.books
        .sort_by(|a, b| natural_cmp(&a.title, &b.title).then_with(|| a.path.cmp(&b.path)));
    Ok(scan)
}

fn scan_dir(dir: &Path, depth: u32, out: &mut Scan) -> std::io::Result<()> {
    let mut subdirs = Vec::new();
    let mut image_count = 0u32;
    let mut image_size = 0u64;
    for ent in fs::read_dir(dir)? {
        let Ok(ent) = ent else { continue };
        let Ok(meta) = ent.metadata() else { continue };
        let name = ent.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            if depth < 2 {
                subdirs.push(ent.path());
            }
        } else if meta.is_file() {
            if is_zip_name(&name) {
                // ルート直下の zip も深さ1の冊として拾う
                out.books.push(BookEntry {
                    path: ent.path(),
                    kind: BookKind::Zip,
                    mtime_ns: mtime_ns(&meta),
                    size: meta.len(),
                    title: stem(&name),
                });
            } else if depth > 0 && is_image_name(&name) {
                image_count += 1;
                image_size += meta.len();
            }
        }
    }
    if image_count > 0 {
        let meta = fs::metadata(dir)?;
        out.books.push(BookEntry {
            path: dir.to_path_buf(),
            kind: BookKind::Folder,
            mtime_ns: mtime_ns(&meta),
            size: image_size,
            title: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        });
    }
    for sub in subdirs {
        // 読めない子ディレクトリは飛ばして走査を続け、飛ばしたことは記録する（#5）
        if let Err(err) = scan_dir(&sub, depth + 1, out) {
            out.skipped.push(Skipped {
                path: sub,
                error: err.to_string(),
            });
        }
    }
    Ok(())
}

fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}
