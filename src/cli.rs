//! CLI（設計 §3.3）。
//!
//! * `manga-viewer index <root> [--db <path>] [--thumbs]` — 走査して索引 DB に反映し、件数と所要を出す。
//!   `--thumbs` で表紙サムネイルも作る
//! * `manga-viewer list [--db <path>]` — 索引 DB の中身を TSV で出す
//! * `manga-viewer view [--book ID] [--page N]` — 単独ウィンドウで 1 冊を読む（p0-6b）
//! * `manga-viewer view --path <zip|dir>` — 索引なしで直接開く
//! * 引数なし — 索引に冊があれば一覧（#1）、無ければ空の窓。zip / フォルダをドロップしても開ける
//!
//! ルートは位置引数のほか `--root` でも書ける（設計 §3.3 の形）。両方あれば位置引数が勝つ。

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::index::{self, BookRow, Index, Status};

/// 索引 DB の既定パスを決めてから CLI を走らせる（`main` から呼ぶ）。
///
/// エラーは `Err` で返すので、呼び手が `caused by` を出して終了コード 1 で終わればよい。
pub fn run(root: Option<PathBuf>, db: Option<PathBuf>, command: &Command) -> anyhow::Result<()> {
    let db = match db {
        Some(path) => path,
        None => index::default_db_path()?,
    };
    execute(root, &db, command)
}

/// DB の既定パスを決えずに走らせる（試験用）。
pub fn execute(root: Option<PathBuf>, db: &Path, command: &Command) -> anyhow::Result<()> {
    match command {
        Command::Index {
            root: positional,
            thumbs,
        } => {
            let root = positional
                .clone()
                .or(root.clone())
                .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_ROOT));
            let mut index = Index::open(db)?;
            let report = index.reindex(&root)?;
            // p0-1a からの出力契約（一覧の要約）。頁数は DB の行から数える
            let rows = index.books_under(&root)?;
            let pages: i64 = rows.iter().map(|row| row.page_count).sum();
            let broken = rows
                .iter()
                .filter(|row| row.status == Status::Broken)
                .count();
            println!("root\t{}", root.display());
            println!("db\t{}", db.display());
            println!("books={} pages={} broken={}", rows.len(), pages, broken);
            println!(
                "discovered={} added={} updated={} unchanged={} removed={} broken={} rescanned={} skipped={} elapsed_ms={}",
                report.discovered,
                report.added,
                report.updated,
                report.unchanged,
                report.removed,
                report.broken,
                report.rescanned(),
                report.skipped.len(),
                report.elapsed.as_millis(),
            );
            // 読めずに飛ばした子ディレクトリは stderr に理由（#5: 冊が黙って欠けないように）
            for skipped in &report.skipped {
                eprintln!(
                    "skipped	{}	{}",
                    skipped.path.display(),
                    flat(&skipped.error)
                );
            }
            // 壊れた冊は stderr に理由（設計 A-6: 一覧には出る、開けない）
            for row in rows.iter().filter(|row| row.status == Status::Broken) {
                eprintln!(
                    "broken\t{}\t{}",
                    row.path.display(),
                    flat(&row.error.clone().unwrap_or_default())
                );
            }
            if *thumbs {
                let covers = index.generate_covers(Some(&root), None)?;
                println!(
                    "covers missing={} generated={} failed={} elapsed_ms={}",
                    covers.missing,
                    covers.generated,
                    covers.failed,
                    covers.elapsed.as_millis(),
                );
            }
        }
        Command::View {
            book,
            path,
            page,
            exit_after_ms,
        } => {
            let (title, book) = match path {
                Some(path) => crate::view::open_path(path)?,
                None => crate::view::open_from_index(db, root.as_deref(), *book)?,
            };
            let mut app = crate::view::ViewApp::new(
                title,
                book,
                *page,
                exit_after_ms.map(std::time::Duration::from_millis),
            );
            if path.is_none() {
                app = app.with_index(db.to_path_buf(), root.clone());
            }
            crate::view::launch(app)?;
        }
        Command::List => {
            let index = Index::open(db)?;
            for row in index.books()? {
                print_row(&row);
            }
        }
    }
    Ok(())
}

/// `list` の 1 行（TSV: id / title / kind / page_count / status / path）。
///
/// 改行とタブは潰す（TSV の行を壊さない）。既定では題名の自然順。
fn print_row(row: &BookRow) {
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        row.id,
        flat(&row.title),
        row.kind.as_str(),
        row.page_count,
        row.status.as_str(),
        flat(&row.path.to_string_lossy()),
    );
}

/// TSV と 1 行出力を守るため、タブと改行はスペースに潰す。
fn flat(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

#[derive(Parser)]
#[command(
    name = "manga-viewer",
    about = "ローカルで動く高速漫画ビューア（p0 実装中）"
)]
pub struct Cli {
    /// 走査するルート（`index` に指定しなければこれ、どちらも無ければ既定の D:\books）
    #[arg(long, global = true, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// 索引 DB のパス（既定は %LOCALAPPDATA%\manga-viewer\index.db。`MANGA_VIEWER_DATA` でも変えられる）
    #[arg(long, global = true, value_name = "PATH")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// ルートを走査して索引 DB に反映する（件数と所要を出す）
    Index {
        /// 走査するルート（既定は D:\books）
        #[arg(value_name = "ROOT")]
        root: Option<PathBuf>,
        /// 索引の後に、表紙が無い冊の表紙サムネイルを作る
        #[arg(long)]
        thumbs: bool,
    },
    /// 索引 DB の中身を出す（TSV: id / title / kind / page_count / status / path）
    List,
    /// 索引の冊を単独ウィンドウで読む（←/→ で頁送り、Esc で終了）
    View {
        /// 冊の id（`list` の先頭列。省略なら最初の壊れていない冊）
        #[arg(long, value_name = "ID")]
        book: Option<i64>,
        /// 索引を使わず zip / フォルダを直接開く（DB 不要。`--book` とは併用不可）
        #[arg(long, value_name = "PATH", conflicts_with = "book")]
        path: Option<PathBuf>,
        /// 開く頁（0 始まり）。頁数以上なら最後の頁に丸める
        #[arg(long, default_value_t = 0)]
        page: usize,
        /// 指定ミリ秒で自動的に閉じる（生存確認用）
        #[arg(long, value_name = "MS", hide = true)]
        exit_after_ms: Option<u64>,
    },
}
