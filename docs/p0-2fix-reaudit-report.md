# MV-p0-2fix 再監査報告（commit 12251a4）

判定: **合格**（所見は軽微 2 件のみ。どちらも現段階で実害なし）

対象: branch qwen-p0、HEAD 12251a4。作業者の申告は信用せず、すべて自分で再測した。
検査データは合成のみ（`tests/.tmp/p0-2reaudit/`）。`D:\books`・`D:\torrent` には触れていない。

## 1. 4 門（独立再測）

| 門 | 結果 |
|---|---|
| `cargo fmt --check` | exit 0 |
| `cargo clippy -q --all-targets` | exit 0・警告なし |
| `cargo test` | 全通過。lib 10 + tests/index_db.rs 16 + tests/scan_book.rs 7 = **33 本**（申告 33 と一致） |
| `cargo build` | exit 0 |

## 2. 変異の再実施

mutate.py の M1〜M7 と同じものに、自作の追加 7 本を加えた（`tests/.tmp/p0-2reaudit/mut.py`）。
各変異のあと src/index.rs を元に戻し、`git status` が clean であることを確認した。

| 結果 | 本数 |
|---|---|
| 落ちた | **15 / 16** |
| 生存 | **1 / 16** |

- 前回の生存 M1〜M7（差分判定 4 種・削除整理停止・削除範囲・表紙削除・自然順・user_version）は **9/9 すべて落ちた**（M2 系を含む）。
- 追加の変異で落ちたもの:
  - key の小文字化除去
  - 区切り `\`→`/` 正規化の除去
  - 末尾スラッシュ除去の除去
  - `ON CONFLICT(key)`→`ON CONFLICT(path)`
  - `under_root` の境界判定（`/` 確認）除去
  - NotADatabase 案内文の変更
- **生存 1 本: N7 `PRAGMA foreign_keys=ON`→`OFF`**。現段階では `covers` に書き込む処理がまだ無い（p0-2c で実装予定）ため、`ON DELETE CASCADE` は観測できない。実害はない。ただし p0-2c で表紙を書き始めたら「冊を消すと covers も消える」試験が必要。

## 3. 大小文字重複の再現

`Lib` / `LIB` / `/` 区切りの小文字 `lib` の 3 通りで同じ DB に再索引した。

- 1 回目: added=2、books=2 pages=5。
- 2 回目・3 回目: added=0 / unchanged=2 / removed=0、books=2 pages=5。
- DB を直接見ると 2 行・page_count 合計 5 で、`key` は小文字化・`/` 正規化済み、`path` は元の綴りのまま。
- 結果: **重複しない。ページ数と行数も一致する。**

## 4. 非 SQLite DB

テキストファイルを `--db` に指定して `index` と `list` を実走した。

- 両方とも exit 1。スタックトレースなし。
- 表示は「エラー: 索引 DB が壊れています（SQLite のファイルではありません）: <path>」と「削除するか、`--db` で別のファイルを指定してください」。
- 元のファイルは書き換えられていない。

## 5. SCHEMA_VERSION 2 移行

`user_version=1` の旧 DB（旧スキーマ・1 行入り）に対して `list` を実走した。

- exit 0。`user_version` が 2 になり、books は 0 行に作り直され、`key` 列が追加された。
- 続けて `index` を実走すると 2 冊が正常に追加された。
- 設計 §2.2 の「移行は作らず捨てて作り直す」どおり。

## 所見

1. **（軽微）v1 → v2 の作り直しが無言。** 旧 DB の行は黙って捨てられ、メッセージが出ない。索引は再生成できるので実害はないが、`list` が空になって戸惑う可能性はある。必要なら「索引を作り直しました」を 1 行 stderr に出すとよい。
2. **（軽微）N7 生存**（上記）。p0-2c で `covers` の CASCADE 試験を足すこと。

## 補足

- 追加の変異を入れた直後に、バイナリを元ソースで再ビルドしてから CLI 実走した。再ビルド前の実走では、変異した案内文が出力に混ざっていた。再ビルド後の結果だけを §3〜§5 に記載している。
- 試験は一時ディレクトリの DB だけを使い、実 DB には書かない（`cli_index_summarizes` も一時 DB。`MANGA_VIEWER_DATA` は env_remove）。
