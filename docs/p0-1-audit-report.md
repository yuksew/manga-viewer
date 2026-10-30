# MV-p0-1 監査報告

- 日付: 2026-10-30
- 監査対象: `F:\work\manga-viewer`（HEAD `731ed83`、ブランチ `master` のみ）
- **判定: 直してから（監査不能 = 実装が存在しない）**

## 最も重大な 3 件

1. **p0-1 の実装が作業ツリーにも履歴にも無い。** `src/` は `main.rs`（`Hello, world!`）だけ。`lib.rs`・`scan`・`book`・`index`・自然順関数・CLI（`index` / `list`）のいずれも無い。ブランチ・stash・worktree も `master` の 1 本のみで、別の場所に実装が隠れている形跡は無い。
2. **`docs/p0-1-implementation.md`（実装の記録）が無い。** 申告値そのものが無いので、申告と実測の突き合わせもできない。
3. **`tests/` が無い。** 合成データ用の `tests/common/mod.rs`、単体・結合テスト、B-3(a,b) のベンチ（`index_first100_ms` / `index_all_ms`）がすべて未作成。`Cargo.toml` の `[dependencies]` も空で、p0-0（依存の関門）すら終わっていない。設計 §5 では p0-1a・p0-1b より前に p0-0 を通す手順なので、手順の前提も満たしていない。

## 確認したこと

| 確認 | 結果 |
| --- | --- |
| `git log --all` | 3 コミット（初期化 / 案内 / p0 設計）のみ。p0-1 に当たるコミット無し |
| `git status` | clean。未コミットの実装も無い |
| `src/` `tests/` `docs/` | `src/main.rs`、`docs/p0-design.md`、`docs/project-guide.md` のみ。`tests/` は存在しない |
| `Cargo.toml` | 依存 0 件（`edition = "2024"`） |
| 漫画ファイル | `D:\books` `D:\torrent` には触れていない |

## 実施していないこと

- **`cargo fmt --check` / `clippy` / `test` / `build`:** この監査セッションでは `cargo` を承認なしで実行できない（許可は python/git/node/npm/godot 系のみ）。対象が Hello world なので、回しても p0-1 の合否には何も言えず、省略した。
- **設計 §4 の合格条件の実行確認・変異試験・境界試験:** 対象コードが無いため不能。具体的には次を叩けていない。
  - 「一覧に出るが開けない」冊（broken）
  - 自然順（`2.png` < `10.png`）
  - mtime/size 変更での再スキャン
  - 深さ 3 の冊を拾わないこと
  - 空フォルダを出さないこと
  - `user_version` 破壊での作り直し

## 次に必要なこと

- 実装側が本当にコミットしたのか確認する。別クローン（`C:\work\manga-viewer` を含む）や別ブランチに載っていて、このリポジトリへ未反映の可能性がある。依頼文は `C:\work\manga-viewer` と書いているが、監査の許可範囲は `F:\work\manga-viewer` のみ。実装が C: 側にあるなら、`F:` へ取り込んでから再監査する。
- 実装が無いなら、p0-0 → p0-1a → p0-1b の順にカードを出し直す。
- 再監査では、設計 §4 の合格条件の実行確認、変異試験 3〜5 本、境界試験を上記の方針で行う。
