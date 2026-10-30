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
        // 設計 §3.3 の引数なし = GUI。p0-3a まで未実装なので案内を出す
        None => {
            let db = match &cli.db {
                Some(path) => path.clone(),
                None => manga_viewer::index::default_db_path()?,
            };
            cli::gui_stub(cli.root.as_deref(), &db);
            Ok(())
        }
    }
}
