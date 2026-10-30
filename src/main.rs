use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use manga_viewer::book::Book;
use manga_viewer::scan::scan_root;

#[derive(Parser)]
#[command(name = "manga-viewer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// ルートを走査して冊数と頁数の要約を出す
    Index { root: PathBuf },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Index { root } => index(&root),
    }
}

fn index(root: &Path) -> anyhow::Result<()> {
    let books = scan_root(root)?;
    let (mut pages, mut broken) = (0usize, 0usize);
    for b in &books {
        match Book::open(&b.path, b.kind) {
            Ok(book) => pages += book.page_count(),
            Err(e) => {
                broken += 1;
                eprintln!("broken\t{}\t{e}", b.path.display());
            }
        }
    }
    println!("books={} pages={pages} broken={broken}", books.len());
    Ok(())
}
