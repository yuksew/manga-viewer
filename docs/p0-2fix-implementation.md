# p0-2fix — 監査（p0-2a / p0-2b）の重大所見を直す

- 日付: 2026-10-31 / branch `qwen-p0`
- 元: `docs/p0-2a-audit-report.md`、`docs/p0-2b-audit-report.md`、`docs/p0-2b-audit-report-2.md`
- 検査データは合成のみ（`D:\books` `D:\torrent` には触れていない）。

## 変えたもの

1. **索引層の試験を追加**（`tests/index_db.rs`、16 本。先に書いて RED を確認してから実装を触った）
   - 2 回目 index で `rescanned=0` / mtime だけ・size だけの変更で `updated=1` / 削除で `removed=n` /
     別ルート（接頭辞だけ同じ隣も）を巻かない / `reindex_upto` は削除しない /
     一覧は挿入順でなく自然順 / `books_under` の範囲 / broken が理由つきで残り 2 回目は `unchanged` /
     更新した冊の表紙だけ失効 / `user_version` 不一致で作り直し・一致なら保持 /
     大小文字違いのルートで行が増えない（Windows）/ 非 SQLite の DB / CLI の `list`。
2. **大小文字違いのルートで行が重複**: `books` に `key`（`path_key` = 区切り統一・小文字化、UNIQUE）を足し、
   `ON CONFLICT(key)` で突合。`path` は最後に見た綴りで更新。`DELETE_COVER` も `key` で引く。
   スキーマが変わるので `SCHEMA_VERSION` を 1 → 2（旧 DB は既存の仕組みで作り直される）。
3. **非 SQLite の index.db**: 自動で消さず（利用者の別ファイルを `--db` に渡した誤操作で消さないため）、
   `IndexError::NotADatabase` で「索引 DB が壊れています … 削除するか、`--db` で別のファイルを指定してください」
   と案内して終了コード 1。ファイルは書き換えない。`file is not a database` は出ない。
4. **`cli_index_summarizes` が実 DB に書く**: `main` の DB 解決は `--db` → `MANGA_VIEWER_DATA` → `%LOCALAPPDATA%` で、
   どちらも無いと実 DB になる。試験は `--db` に一時ファイルを渡し、`MANGA_VIEWER_DATA` も外す。
   新しい CLI 試験も同様。
   なお過去の `cargo test` が実 DB に混ぜた分は、作業範囲外のため確認・掃除していない（利用者側で `index.db` を消せば再索引できる）。

## 変異試験（`tests/.tmp/p0-2fix/mutate.py`、`src/index.rs` を一時改変して `cargo test`）

| 変異 | 監査 | 今回 |
| --- | --- | --- |
| M1 差分判定を常に偽（毎回再走査） | 生存 | 落ちた |
| M2 mtime 比較を外す | 生存 | 落ちた |
| M2b size 比較を外す（追加） | — | 落ちた |
| M2c 差分判定を常に真（追加） | 生存 | 落ちた |
| M3 削除整理を止める | 生存 | 落ちた |
| M4 削除範囲（`under_root`）を外す | 生存 | 落ちた |
| M5 更新時の古い表紙削除を外す | 生存 | 落ちた |
| M6 自然順ソートを外す | 生存 | 落ちた |
| M7 `user_version` 不一致でも作り直さない | 生存 | 落ちた |

実施後、`src/index.rs` は元に戻してある。

## 4 門

fmt --check / clippy -q --all-targets / test / build すべて exit 0。
テスト本数: 単体 10 + 結合 7（before）→ 単体 10 + 結合 7 + 索引層 16 = 33 本（after、+16）。

## 残し（今回の範囲外）

- decode モジュール（設計 p0-2a）と段名の食い違い。
- フォルダ冊の鍵が同サイズの上書きを見ない件（設計 §7-8 で許容、p0-2c の表紙に影響）。
- 存在しないルートのエラーに path が出ない件、記録の `root=` が実際は `root<TAB>` の件。
