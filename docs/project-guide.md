# manga-viewer — 高速漫画ビューア（新規プロジェクト）

- 場所: `C:\work\manga-viewer`（git repo、branch `main`）。2026-10-30 初期化。
- 言語: Rust。ツールチェーンは rustup（`~\.cargo\bin` に `cargo 1.99.1` / `rustc 1.99.1`）。
  **PATH に `%USERPROFILE%\.cargo\bin` が入っていないシェルがある** — ツールは絶対パスか PATH 追加で呼ぶ。

## 検証の門（この順。全部ローカルで動く）

```
cargo fmt --check
cargo clippy -q
cargo test
cargo build
```

- 全部 exit 0 が合格線。`cargo run` は `Hello, world!` を出すだけ（まだ画面なし）。
- MSVC のリンカ（`cl.exe`）は未確認。今は `cargo build` が通っているが、
  依存 crate を足してリンクが落ちたら Windows SDK/Visual Studio Build Tools の導入を先に疑う。

## 慣習

- コミットは日本語の一行文（image-reviewer と同じ流儀）。`target/` は `.gitignore` 済み。
- 報告・設計・監査の文書は `docs/` に `<段>-<種別>.md`（例: `p0-1-implementation.md`）。
- ワークツリーで回すときは cargo のキャッシュをワークツリーごとに持つ（`target/` を共有しない）。

## 状態（2026-10-30）

- 実装はまだ無し。cargo 初期化 + git 初期コミット `a5c7f99` のみ。
- p0 の設計は `docs/p0-design.md`（egui/eframe、小段 p0-0〜p0-4）。次は p0-0（依存の関門）から Qwen へ渡す。
