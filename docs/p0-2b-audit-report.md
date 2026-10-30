# MV-p0-2b 監査報告 — 索引の SQLite 永続化

対象: branch `qwen-p0`、HEAD `0fc3a2b`（p0-2b）。実装記録は `docs/p0-2b-implementation.md`、設計は `docs/p0-design.md`。
検査は合成データ（`tests/.tmp/root`、50 冊 × 20 頁。フォルダと zip が半々）だけで行った。`D:\books`・`D:\torrent` には触れていない。

## 判定: **条件つきで使える**

実走の挙動は申告どおり。ただし index 層を守る試験が実質ゼロで、大文字小文字違いのルートで行が重複する不具合がある。次の段（p0-2c / p0-3a）に進む前に下の 3 件を直すこと。

## 4 門（自分で回した）

| 門 | 結果 |
|---|---|
| `cargo fmt --check` | 通過 |
| `cargo clippy -q` | 警告なし |
| `cargo test` | 通過（単体 10 / 統合 7） |
| `cargo build` | 通過 |

## CLI 実走

- 1 回目 `index` → `books=50 pages=1000 broken=0`、`added=50 rescanned=50`
- 2 回目 → `unchanged=50 rescanned=0`（`elapsed_ms=2`）
- `list` → 50 行（題名の自然順）。司令塔の実測値と同じ。

## 境界

| 操作 | 結果 |
|---|---|
| 空 DB に `list` | 0 行、エラーなし。OK |
| 冊を 2 つ削除して再索引 | `removed=2`、`books` から消える。OK |
| 空 zip を追加 | broken として一覧に出る（理由「頁が 0 枚です」）。OK |
| 画像なしフォルダ・空フォルダ | 冊として拾われない（p0-1a と同じ挙動）。OK |
| ルートを大文字小文字違いで指定、変更なし | `unchanged=49`。OK |
| ルートを大文字小文字違いで指定、1 冊に頁を追加 | **重複行ができる**（所見 2） |

## 変異試験（3 本とも生き残り＝試験が守っていない）

| 変異 | 結果 |
|---|---|
| 差分索引の mtime/size 比較を外す（`&& false`） | `cargo test` 全通過 |
| 削除冊の整理（`if limit.is_none()` 節）を無効化 | `cargo test` 全通過 |
| `books()` の自然順ソートを外す | `cargo test` 全通過 |

「頁順のキー行」: 頁は DB に入らず `books.page_count` だけなので（設計 §2.2 どおり）、該当する行は無い。代わりに一覧順のソートで試した。

## 重大な所見 3 件

1. **index 層の試験がない**。`reindex`・`Index::open`・`books_under` を呼ぶ試験は 1 本もない（`tests/` を grep して 0 件）。単体 5 本は `path_key`・`under_root` などの小さな関数だけ。差分索引・削除の整理・一覧順は、上の変異 3 本が全部通ったとおり、壊しても気づけない。`docs/p0-2b-implementation.md` が謳う合格条件（`rescanned=0`、削除の整理）は、今は手で実走して確かめるしかない。
2. **ルートの大文字小文字が違うと重複行ができる**（再現済み）。差分判定は `path_key`（小文字化）で突き合わせるが、`UPSERT_BOOK` の `ON CONFLICT(path)` は DB に入れた生の文字列を大文字小文字を区別して比べる。`index F:/WORK/.../ROOT` のように別の綴りで指定し、かつ中身が更新された冊があると、新しい綴りで別行が挿入される。旧行は `alive` に鍵が入っているため削除されない。実測は `discovered=49` なのに `books=50 pages=981`（正しくは 960 + 更新分）。`index` の要約と `list` が二重に数える。対策は、更新時に既存行の `id` で `UPDATE` するか、`path` 列を `COLLATE NOCASE` にするか、DB に入れる前に綴りを揃える。
3. **`cli_index_summarizes` が実利用の DB に書く疑い**。この試験は `--db` も `MANGA_VIEWER_DATA` も渡さずに `index` を走らせる（`tests/scan_book.rs:145`）。`default_db_path()` の既定は `%LOCALAPPDATA%\manga-viewer\index.db` なので、`cargo test` のたびに一時フォルダの冊が実 DB に混ざるはず。コードから読んだ推測で、作業範囲外のため実機の DB は開いていない。対策は試験側で `--db` に一時ファイルを渡す。

## 軽微（参考）

- 冊が 1 つ見つかるたびに `existing_rows()` が全行を読んでソートする（`books()` が毎回 `natural_cmp` で並べる）。1 回の索引につき 1 回なので今は支障ない。冊数が万単位になれば、ソートを `existing_rows` から外すこと。
- 試験用の `tests/.tmp/gen.py`（gitignore 対象）は手元の生成用で、リポジトリには入れていない。
