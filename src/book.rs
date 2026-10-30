use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use zip::ZipArchive;

use crate::natural::natural_cmp;
use crate::scan::BookKind;

#[derive(Debug, thiserror::Error)]
pub enum BookError {
    #[error("読めません: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP の中央ディレクトリを読めません: {0}")]
    Zip(String),
    #[error("暗号化された ZIP は非対応です")]
    Encrypted,
    #[error("頁が 0 枚です")]
    NoPages,
    #[error("頁番号が範囲外です: {0}")]
    PageOutOfRange(usize),
}

const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

fn ext_lower(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
}

pub fn is_zip_name(name: &str) -> bool {
    matches!(ext_lower(name).as_deref(), Some("zip" | "cbz"))
}

/// 頁として数える名前か（拡張子・ドットファイル・Thumbs.db・`__MACOSX/` を見る）。
pub fn is_image_name(path: &str) -> bool {
    let path = path.replace('\\', "/");
    if path.split('/').any(|c| c == "__MACOSX") {
        return false;
    }
    let base = path.rsplit('/').next().unwrap_or("");
    if base.starts_with('.') || base.eq_ignore_ascii_case("Thumbs.db") {
        return false;
    }
    ext_lower(base).is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()))
}

enum Source {
    Zip(Mutex<ZipArchive<File>>),
    Folder(PathBuf),
}

struct Page {
    /// `/` 区切りの冊内パス
    name: String,
    /// ZIP のエントリ番号（フォルダでは未使用）
    index: usize,
}

pub struct Book {
    source: Source,
    pages: Vec<Page>,
}

impl Book {
    pub fn open(path: &Path, kind: BookKind) -> Result<Book, BookError> {
        let mut pages = Vec::new();
        let source = match kind {
            BookKind::Zip => {
                let file = File::open(path)?;
                let mut archive =
                    ZipArchive::new(file).map_err(|e| BookError::Zip(e.to_string()))?;
                for i in 0..archive.len() {
                    let entry = archive
                        .by_index_raw(i)
                        .map_err(|e| BookError::Zip(e.to_string()))?;
                    if entry.is_dir() {
                        continue;
                    }
                    let name = entry
                        .name()
                        .map_err(|e| BookError::Zip(format!("パスが UTF-8 ではありません: {e}")))?
                        .replace('\\', "/");
                    if !is_image_name(&name) {
                        continue;
                    }
                    if entry.encrypted() {
                        return Err(BookError::Encrypted);
                    }
                    pages.push(Page { name, index: i });
                }
                Source::Zip(Mutex::new(archive))
            }
            BookKind::Folder => {
                for ent in fs::read_dir(path)? {
                    let ent = ent?;
                    let name = ent.file_name().to_string_lossy().into_owned();
                    if ent.metadata()?.is_file() && is_image_name(&name) {
                        pages.push(Page { name, index: 0 });
                    }
                }
                Source::Folder(path.to_path_buf())
            }
        };
        if pages.is_empty() {
            return Err(BookError::NoPages);
        }
        pages.sort_by(|a, b| natural_cmp(&a.name, &b.name).then_with(|| a.name.cmp(&b.name)));
        Ok(Book { source, pages })
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// 頁順（自然順）に並んだ冊内パス。
    pub fn page_names(&self) -> Vec<&str> {
        self.pages.iter().map(|p| p.name.as_str()).collect()
    }

    pub fn read_page_bytes(&self, i: usize) -> Result<Vec<u8>, BookError> {
        let page = self.pages.get(i).ok_or(BookError::PageOutOfRange(i))?;
        match &self.source {
            Source::Zip(archive) => {
                let mut archive = archive.lock().unwrap_or_else(|e| e.into_inner());
                let mut entry = archive
                    .by_index(page.index)
                    .map_err(|e| BookError::Zip(e.to_string()))?;
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf)?;
                Ok(buf)
            }
            Source::Folder(dir) => Ok(fs::read(dir.join(&page.name))?),
        }
    }
}
