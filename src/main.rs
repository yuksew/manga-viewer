use std::process::ExitCode;

use clap::Parser;
use manga_viewer::cli::{self, Cli};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("エラー: {err}");
            for cause in err.chain().skip(1) {
                eprintln!("  理由: {cause}");
            }
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: &Cli) -> anyhow::Result<()> {
    match &cli.command {
        Some(command) => cli::run(cli.root.clone(), cli.db.clone(), command),
        // 引数なし = 索引に冊があれば一覧、無ければ空の窓（zip / フォルダのドロップで開く）
        None => {
            let db = match &cli.db {
                Some(db) => db.clone(),
                None => manga_viewer::index::default_db_path()?,
            };
            manga_viewer::view::run_start(&db, cli.root.as_deref())
        }
    }
}
