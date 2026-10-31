use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use manga_viewer::cli::{self, Cli};

fn main() -> ExitCode {
    // 起動の測定（B-2）の起点。引数解析より前に取る
    let t0 = Instant::now();
    let cli = Cli::parse();
    match dispatch(&cli, t0) {
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

fn dispatch(cli: &Cli, t0: Instant) -> anyhow::Result<()> {
    if cli.autopilot.is_some() && cli.command.is_some() {
        anyhow::bail!("--autopilot は引数なし起動（一覧）でだけ使えます");
    }
    match &cli.command {
        Some(command) => cli::run(cli.root.clone(), cli.db.clone(), command),
        // 引数なし = 索引に冊があれば一覧、無ければ空の窓（zip / フォルダのドロップで開く）
        None => {
            let db = match &cli.db {
                Some(db) => db.clone(),
                None => manga_viewer::index::default_db_path()?,
            };
            manga_viewer::view::run_start(&db, cli.root.as_deref(), t0, cli.autopilot)
        }
    }
}
