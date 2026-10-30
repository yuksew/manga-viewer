# MV-p0-4fix 実装 — imgcheck 生存変異 5 本へのテスト追加（Claude）

対象: branch `qwen-p0`。起点は commit b981d25（監査報告 docs/p0-4-audit-report.md）。
漫画ファイルには触れていない。テストは `tests/common` の合成データのみ。
**src・Cargo.toml は無変更**（tests と docs のみ）。

## 追加したテスト（tests/image_check.rs、5 本）

| 変異 | テスト | 内容 |
|---|---|---|
| M05 | `webp_requires_riff_magic` | 正常 WebP は通り、先頭を `RIFX` にすると拒否 |
| M07 | `ffd8_inside_body_does_not_make_a_jpeg` | 本体（GIF はトレーラ手前、WebP はペイロード末尾）に `FF D8` を含む正常品が GIF / WebP と判定される |
| M09 | `gif_of_13_bytes_ending_in_trailer_is_rejected` | 13 バイトで末尾 `3B`（幅高さ非 0）の GIF を拒否 |
| M11 | `webp_chunk_overrunning_riff_by_one_byte_is_rejected` | チャンク長 +1 は拒否、ちょうどは通る（境界の両側） |
| M17 | `png_ihdr_length_other_than_13_is_rejected` | IHDR 長さ 14 + 本体 1 バイト追加（後続チャンクの並びは保つ）を拒否 |

## RED → GREEN

src を変えずテストだけ足すので、HEAD に対しては最初から GREEN になる。
RED は「変異体に対して落ちること」で確認した（下表）。
HEAD に対する `cargo test --test image_check`: 11 passed（6 → 11）。

## 変異の再撃墜（実測）

ハーネス `tests/.tmp/mut5.py`（gitignore 済み）。変異を 1 本ずつ src/imgcheck.rs に当て、
`cargo test --test image_check` を走らせ、必ず元のバイト列へ書き戻す。

| # | 変異 | exit | 落ちた本数 | 落ちたテスト |
|---|---|---|---|---|
| M05 | dispatch から `&b[..4] == b"RIFF"` を削除 | 101 | 1 | `webp_requires_riff_magic` |
| M07 | JPEG 判定を `starts_with` → 先頭以外でも一致（`windows(2).any`） | 101 | 1 | `ffd8_inside_body_does_not_make_a_jpeg` |
| M09 | GIF `len < 14` → `< 13` | 101 | 1 | `gif_of_13_bytes_ending_in_trailer_is_rejected` |
| M11 | WebP `chunk + 20` → `+ 19` | 101 | 1 | `webp_chunk_overrunning_riff_by_one_byte_is_rejected` |
| M17 | PNG `be32(b, 8) != Some(13)` を削除 | 101 | 1 | `png_ihdr_length_other_than_13_is_rejected` |

5 本とも、新テストのうち対応する 1 本だけが落ちた（ほかの既存テストは通ったまま）。
等価変異 M06・M08 は対象外。

## 4 門（独立実測）

| 門 | exit |
|---|---|
| `cargo fmt --check` | 0 |
| `cargo clippy -q --all-targets` | 0 |
| `cargo test` | 0 |
| `cargo build` | 0 |

テスト本数: **47 → 52**（lib 10 + image_check 11 + index_db 16 + scan_book 7 + server 8）。

## 作業ツリー

変異はすべて書き戻し済み。`git diff -- src` は空。コミットは tests/image_check.rs と本 docs のみ。
