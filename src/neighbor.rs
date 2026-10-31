//! 次冊/前冊の候補解決（純粋関数）。窓の確認プロンプトで「はい」になったときに使う。
//!
//! 索引 DB があればその順（題名の自然順）で、無ければ同じフォルダ内の自然順で隣を探す。

use std::path::{Path, PathBuf};

use crate::book::{is_image_name, is_zip_name};
use crate::index::{BookRow, Status};
use crate::natural::natural_cmp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDir {
    Prev,
    Next,
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// 索引順 `rows` で `cur` の隣の壊れていない冊。`cur` が索引に無ければ `None`。
#[must_use]
pub fn neighbor_in_rows(rows: &[BookRow], cur: &Path, dir: NavDir) -> Option<PathBuf> {
    let pos = rows.iter().position(|r| same_path(&r.path, cur))?;
    let ok = |r: &&BookRow| r.status != Status::Broken;
    match dir {
        NavDir::Next => rows[pos + 1..].iter().find(ok),
        NavDir::Prev => rows[..pos].iter().rev().find(ok),
    }
    .map(|r| r.path.clone())
}

/// 冊として開けそうなエントリか（zip/cbz、または画像を直下に持つフォルダ）。
fn is_book_candidate(path: &Path) -> bool {
    if path.is_dir() {
        return std::fs::read_dir(path).is_ok_and(|rd| {
            rd.flatten()
                .any(|e| is_image_name(&e.file_name().to_string_lossy()) && e.path().is_file())
        });
    }
    path.to_str().is_some_and(is_zip_name)
}

/// `cur` と同じフォルダ内の、自然順で隣の冊（zip/cbz または画像フォルダ）。
#[must_use]
pub fn neighbor_in_dir(cur: &Path, dir: NavDir) -> Option<PathBuf> {
    let name = cur.file_name()?;
    let parent = match cur.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut names: Vec<String> = std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter(|e| e.file_name() == name || is_book_candidate(&e.path()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by(|a, b| natural_cmp(a, b).then_with(|| a.cmp(b)));
    let pos = names
        .iter()
        .position(|n| name.to_string_lossy() == n.as_str())?;
    let next = match dir {
        NavDir::Next => names.get(pos + 1)?,
        NavDir::Prev => names.get(pos.checked_sub(1)?)?,
    };
    Some(parent.join(next))
}

/// 索引が使えて `cur` が載っていればその順、そうでなければフォルダ内の自然順。
#[must_use]
pub fn resolve_neighbor(cur: &Path, dir: NavDir, rows: Option<&[BookRow]>) -> Option<PathBuf> {
    match rows {
        Some(rows) if rows.iter().any(|r| same_path(&r.path, cur)) => {
            neighbor_in_rows(rows, cur, dir)
        }
        _ => neighbor_in_dir(cur, dir),
    }
}
