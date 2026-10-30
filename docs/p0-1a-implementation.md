# p0-1a 実装 — 走査と冊の集合

- 日付: 2026-10-30
- 範囲: 設計 `docs/p0-design.md` §5 の p0-1a のみ（index DB・サムネイル・GUI は未着手）。

## 追加したもの

- `src/lib.rs` — `book` / `natural` / `scan` を公開。
- `src/natural.rs` — `natural_cmp`（大小無視・連続数字は数値・同値なら先頭ゼロが長い方が後）。単体試験 4 本。
- `src/scan.rs` — `scan_root(root) -> Vec<BookEntry{path,kind,mtime_ns,size,title}>`。深さ1・2 を走査。題名の自然順。
- `src/book.rs` — `Book::open(path, kind)`（頁リスト = 拡張子フィルタ + 自然順）、`read_page_bytes(i)`、`BookError`。
- `src/main.rs` — `manga-viewer index <root>`（`books=N pages=M broken=K`、壊れた冊は stderr に理由）。
- `tests/common/mod.rs`（`tests/.tmp/` に作って Drop で消す）、`tests/scan_book.rs`（結合 7 本）。

## 設計から外れた点

1. **p0-0 が未完だった。** 着手時 `Cargo.toml` に依存が無かった。1a に要る分だけ追加した:
   `zip`（`default-features=false, features=["deflate"]`、v9.1.3）、`thiserror`、`anyhow`、`clap`（derive）、
   dev-dependency に `image`（png のみ。合成 PNG 生成用）。`rusqlite` / `eframe` などは未追加なので、
   **rusqlite bundled が通るかの関門は未確認のまま**。`.gitignore` に `tests/.tmp/` を追加。
2. **CLI は `index <root>`（位置引数）**。設計 §3.3 の `index --root <dir>` ではなく、依頼文に従った。`--db` / `list` は 1b。
3. **ルート直下の zip/cbz も冊にした**（深さ1の冊）。A-3 は「ディレクトリの直下」の規則なので、ルート直下の扱いは文言上あいまい。ルート自身は画像があってもフォルダ冊にしない。
4. **ZIP 内の非 UTF-8 パス**: `zip` v9 の `name()` が Result を返すため、非 UTF-8 の ZIP エントリを含む冊は `BookError::Zip`（broken）になる。A-6 の「パスが UTF-8 でない」に合うが、頁単位でなく冊単位で落ちる。
5. **暗号化**: 画像エントリに暗号化があれば `BookError::Encrypted`（試験は無し。合成が難しいため未検査）。
6. フォルダ冊の頁は直下の画像のみ（子ディレクトリの画像は見ない）。
7. 拡張子の判定は名前だけ（先頭バイト判定はデコード側 = p0-2a）。
