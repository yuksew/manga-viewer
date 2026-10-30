# MV-p0-2fix 監査報告

- 日付: 2026-10-31
- 監査対象: `F:\work\mv-qwen`（HEAD `2fdadcb`、ブランチ `qwen-p0`）
- **判定: 直してから（監査不能 = 修正 commit が存在しない）**

## 最も重大な 3 件

1. **t_5a6fa912（MV-p0-2fix）の修正 commit が無い。** `qwen-p0` の HEAD は前回の監査報告 commit（`2fdadcb`）のまま。`0fc3a2b`（p0-2b 実装）以降の commit は docs のみで、`git diff 0fc3a2b HEAD -- src tests Cargo.toml` は空。`git branch -a`・`git worktree list`・`git stash list` を見ても、修正を載せたブランチ・worktree・stash は無い（`master` は `e6fc3af` で p0-1a までしか無い）。
2. **前回の重大所見は未修正のまま。** コードが 1 行も変わっていないので、変異 7/7 生存・大文字小文字重複・非 SQLite DB への案内・`cli_index_summarizes` が一時 DB を使わない件は、いずれも前回の報告どおり残っているはず。ただし今回は再実測していない（手順 2〜4 は対象が無いため省略）。
3. **修正の申告も記録も無い。** 修正側の実装記録（`docs/p0-2fix-implementation.md` 等）が無く、申告値と実測の突き合わせもできない。

## 実測したこと（未修正コードのベースライン）

| 門 | 結果 |
| --- | --- |
| `cargo fmt --check` | 差分なし |
| `cargo clippy -q` | 警告なし |
| `cargo test` | 17 本すべて通過（lib 10 + `tests/scan_book.rs` 7）。前回から本数の増減なし |
| `cargo build` | 成功 |

4 門が通るのは「修正前のコードが前回と同じ状態」という意味でしかなく、修正の合否には何も言えない。

## 実施していないこと

- 変異の再実施（差分判定無効化・削除整理無効化・自然順ソート除去など）: 対象の修正が無いため不能。落ち 0 本 / 生存 0 本ではなく、**未実施**。
- 大小違いルートでの再索引、非 SQLite DB への案内、`cli_index_summarizes` の CLI 実走: 同上。
- 漫画ファイル（`D:\books`・`D:\torrent`）には触れていない。

## 次に必要なこと

- t_5a6fa912 の実装者に、commit 先を確認する。別クローン（`C:\work\mv-qwen` など）や別ブランチにあり、このリポジトリへ未反映の可能性がある。許可範囲は `F:\work\mv-qwen` のみなので、ここへ取り込んでから再監査する。
- 修正が本当に無いなら、前回 3 本の所見を直すカードを出し直す。
- 再監査では、依頼どおり変異 5 本以上の再実施と、大小違いルートの再現手順の再実行を行う。
