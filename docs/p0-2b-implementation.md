# p0-2b 実装 — 索引の永続化（index DB + 差分索引 + CLI）

- 日付: 2026-10-31
- 範囲: 設計 §2.2 の索引キャッシュと §3.3 の CLI `index` / `list`（設計 §5 表では p0-1b の段に相当する中身）。
  頁キャッシュ・先読み・サムネイル（`covers` の中身）は未着手。

## 追加・変更したもの

- `src/index.rs`（新規）— `Index::open` / `reindex` / `books` / `books_under`、`Report`、既定 DB パス決定。単体試験 6 本。
- `src/cli.rs`（新規）— clap derive の `Cli` / `Command`、`index` / `list` の出力、GUI スタブ。
- `src/main.rs` — 上記 CLI に委譲（エラーは `caused by` を出して終了コード 1）。
- `src/scan.rs` — `BookKind::as_str` / `parse`（DB の `kind` 列の往復）。
- `src/lib.rs` — `cli` / `index` を公開。既定ルート `DEFAULT_ROOT = D:\books`（実データ path を書く唯一の場所）。
- `Cargo.toml` — 依存追加: `dirs 7.0.0` / `rayon 1.12.0` / `rusqlite 0.41.1 (bundled)`。

## CLI の使い方

```
manga-viewer index [ROOT] [--db <path>]     # 走査して DB に反映。件数と所要を出す
manga-viewer list  [--db <path>]            # DB 内容を TSV（id/title/kind/page_count/status/path）
manga-viewer                                # 引数なし = GUI（p0-3a まで案内文だけ）
```

- ルートは位置引数か `--root`。どちらも無ければ `D:\books`。`--root` / `--db` はグローバル指定。
- DB 既定は `%LOCALAPPDATA%\manga-viewer\index.db`。`--db` が最優先、無ければ環境変数 `MANGA_VIEWER_DATA` の下 `index.db`（試験用）。
- `index` の stdout は 4 行: `root=…` / `db=…` / `books=N pages=M broken=K`（p0-1a からの契約）/
  `discovered=… added=… updated=… unchanged=… removed=… broken=… rescanned=… elapsed_ms=…`。
  壊れた冊は stderr に `broken<TAB>path<TAB>理由`（設計 A-6: 一覧には出る、開けない）。
- `list` は題名の自然順（同値は path 順）。TSV を壊さないためタブ・改行はスペースに潰す。

## DB スキーマ（`PRAGMA user_version = 1`）

WAL / `synchronous=NORMAL` / `foreign_keys=ON`。`user_version` が既定値と違えば **捨てて作り直す**（移行を作らない設計方針）。

```sql
CREATE TABLE books (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,   -- 絶対パス。鍵はこの正規化形（下記）
    kind TEXT NOT NULL,          -- zip / folder
    mtime_ns INTEGER NOT NULL,
    size INTEGER NOT NULL,
    title TEXT NOT NULL,
    page_count INTEGER NOT NULL, -- 頁リスト自体は入れない（開いたときに作る）
    status TEXT NOT NULL,        -- ok / broken
    error TEXT,                  -- broken の理由
    scanned_at INTEGER NOT NULL  -- epoch 秒
);
CREATE TABLE covers (            -- p0-2c（サムネイル段）が埋める。いまは空
    book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    w INTEGER, h INTEGER, jpeg BLOB
);
```

## 差分索引の挙動

- 突き合わせ鍵は **path**（監査所見 3: 題名は一意でない）。比較用の鍵は `\`→`/`・小文字化・末尾区切り除去で正規化し、
  DB に stored する文字列は正規化前の絶対パス（`\\?\` の verbatim 前置きは剥がす）。
- 走査で見つけた冊ごとに:
  - DB に無く **追加**、`mtime_ns` と `size` が違えば **更新**（ZIP は ZIP 自身の値、フォルダ冊は scan.rs の合計値）。
  - `mtime_ns` と `size` が同じなら **中身を開かない**（`unchanged`）。2 回目以降は `rescanned = added + updated` が 0 になる（設計 §5 の合格線）。
- 開き直す冊だけ rayon で並列に `Book::open`（頁数取得）。DB 書き込みは単一スレッド、**50 冊ごとに 1 トランザクション**の UPSERT。
  更新した冊の古い表紙行は同時に消す。
- 消えた冊は DB から削除。ただし **今回の走査ルートの下だけ**（別ルートの行を巻かない）。
  `reindex_upto(root, limit)`（B-3(a) 測定用）は未走査の冊があるので削除しない。

## 検査結果

合格門（2026-10-31 実測、`%USERPROFILE%\.cargo\bin` を PATH に足して）:

| 門 | exit code |
| --- | --- |
| `cargo fmt --check` | 0 |
| `cargo clippy -q` | 0（警告なし） |
| `cargo test` | 0（単体 10 + 結合 7、failed 0） |
| `cargo build` | 0 |

CLI 実走（run 124/126・司令塔の実測の転記）: 合成ルートで `index` → `books=50 pages=1000 broken=0`、
2 回目 `unchanged=50 rescanned=0`（差分索引が効いている）、`list` → 50 行。

メモ: 着手時の `cargo fmt --check` 不合格は未追跡の新規 2 ファイル側の整形で、`cargo fmt` が `src/cli.rs`（報告どおりの 1 箇所）と
`src/index.rs` に及んだ。track 済みの他ファイルには変化なし。設計 §5 の p0-1b 段にあったベンチ（B-3a/b）はこの段では未実施。
