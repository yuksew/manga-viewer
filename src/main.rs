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
        // 引数なし = 空の窓。zip / フォルダのドロップで開く（DB 不要）
        None => manga_viewer::view::run_empty(None),
    }
}
