# p0-4 実装メモ — 画像ヘッダ検証と `/image` ルート

実装は `src/imgcheck.rs`（検証）と `src/server.rs`（配信）。試験は `tests/image_check.rs`。
Cargo.toml は無変更（デコードライブラリは追加していない）。

## 変更点

| 項目 | 内容 |
|---|---|
| `imgcheck::check(&[u8])` | magic で形式を判定し、ヘッダと終端の整合を見る。`Ok(Kind)` か理由つき `Err` |
| `GET /page/<id>/<n>` | 生バイト透過は維持。返す前に `check` し、壊れていれば 404（本文に「壊れた画像です: 理由」） |
| `GET /image/<id>/<n>` | `/page` と同じ検証・同じ本体。加えて `Content-Disposition: inline; filename="<n+1>.<ext>"` |
| 共通ヘッダ | 画像の 200 には `X-Content-Type-Options: nosniff` |
| `Response.headers` | 追加ヘッダ用の `Vec<(&str, String)>` を足した |

### Content-Type は拡張子ではなく magic で決める（挙動変更）
p0-3 は拡張子から決めていた。`nosniff` を付けると、拡張子が嘘の頁（中身 PNG の `.jpg` など）を
ブラウザが表示しなくなるので、検証で得た実際の形式を返す方針にした。
このため `tests/server.rs::page_bytes_and_range` の 1 箇所（中身 PNG の `x.jpg` が `image/jpeg` →
`image/png`）を更新した。拡張子が未対応の頁は従来どおり 404「未対応」。

### `/image` の Content-Disposition の方針
`inline` で、ファイル名は ASCII だけの `<頁番号>.<拡張子>`（元の名前は使わない）。
非 ASCII のファイル名で `filename*=` のエスケープを実装するリスクを避け、保存時の名前は連番で足りると判断した。

## 検証の中身と限界（std のみ）

| 形式 | 見るもの |
|---|---|
| PNG | 署名、先頭が IHDR（長さ 13・幅高さ非 0）、チャンク長を辿って `IEND`（長さ 0）で**ちょうど**終わる |
| JPEG | `FFD8` 始まり、末尾 `FFD9`（後ろの 0 埋めは許す）、SOS より前に SOF があり幅高さ非 0 |
| GIF | `GIF87a`/`GIF89a`、幅高さ非 0、末尾 `3B` |
| WebP | `RIFF`+`WEBP`、先頭チャンクが `VP8 `/`VP8L`/`VP8X`、RIFF サイズ+8 が実長以下、先頭チャンク長が RIFF 内に収まる |

**限界（デコードしないため）**
- 圧縮データの中身は見ない。ヘッダと終端が正しいまま途中だけ壊れた画像は通る。
- PNG の CRC は見ない。JPEG のスキャン内部は見ない。GIF は中間ブロックを辿らない（寸法と末尾のみ）。
- 見つけられるのは主にトランケイト（途中切れ）・署名違い・寸法 0・終端欠落。
- JPEG で末尾が `FFD9` のまま途中が欠けた場合などは検出できない。

## 試験（47 本。実測: before 41 → after 47、+6）

`cargo test` の各バイナリ: lib 10 / image_check 6 / index_db 16 / scan_book 7 / server 8（+ main・doc は 0）。

- `check_accepts_each_format` — png/jpeg/gif/webp の最小正常画像が通る
- `check_rejects_every_truncation_length` — 4 形式すべてで**全ての切り詰め長**（0..len-1）が拒否される
- `check_rejects_garbage_and_bad_headers` — ごみ、PNG 幅 0、PNG 末尾ごみ、JPEG 寸法 0、GIF 幅 0、WebP チャンク違い。JPEG の 0 埋めは許容
- `per_page_verdicts_across_two_books` — **2 冊・計 8 頁**を 1 頁ずつ個別に判定（a: 良/壊/良、b: 壊/良/良/壊/壊）。
  `/page` と `/image` の両方で、状態・Content-Type・本体バイト列を頁ごとに照合（「最後の 1 件だけ」型の見逃し対策）
- `image_route_is_inline_and_page_route_is_not` — `/image` だけ Content-Disposition、頁番号がファイル名に反映、
  実送出バイト列にもヘッダが載る、範囲外・負数・不明 id は 404
- `content_type_follows_magic_not_extension` — `.jpg` の名で中身 PNG → `image/png`

TDD の順序についての正直な記録: 実装（imgcheck と server の変更）を先に書いてから試験を書いた。
RED（試験が先に失敗する状態）は取っていない。代わりに、既存試験 `page_bytes_and_range` が
Content-Type の仕様変更で失敗することは確認し、意図した変更として更新した。

## 4 門

```
cargo fmt --check            exit 0
cargo clippy -q --all-targets exit 0（警告なし。type_complexity の警告が一度出たので型別名で解消）
cargo test                   exit 0（47 本）
cargo build                  exit 0
```

## serve 実走ログ

合成 cbz のみ（`tests/.tmp/p0-4-live/`、検証後に削除）。
`good.cbz`（正常 PNG 73B ×2 頁）、`mixed.cbz`（1 頁目 正常、2 頁目 末尾 7B 欠落、3 頁目 先頭 20B のみ）。

```
cargo run -q -- index tests/.tmp/p0-4-live/books --db tests/.tmp/p0-4-live/db/index.db
  books=2 pages=5 broken=0
cargo run -q -- serve --port 17941 --db tests/.tmp/p0-4-live/db/index.db --root tests/.tmp/p0-4-live/books
```

python の http.client で取得（ヘッダは主要なものだけ抜粋）:

```
books: {'good': 1, 'mixed': 2}
GET /page/1/0 -> 200 Content-Type=image/png Content-Length=73 nosniff body=73B
GET /page/1/1 -> 200 Content-Type=image/png Content-Length=73 nosniff body=73B
GET /image/1/0 -> 200 image/png 73 nosniff Content-Disposition: inline; filename="1.png" body=73B
GET /image/1/1 -> 200 image/png 73 nosniff Content-Disposition: inline; filename="2.png" body=73B
GET /page/2/0 -> 200 image/png 73 nosniff body=73B
GET /page/2/1 -> 404 text/html; charset=utf-8 Content-Length=277 body=277B     ← 末尾欠落 PNG
GET /page/2/2 -> 404 text/html; charset=utf-8 Content-Length=271 body=271B     ← 20B で切れた PNG
GET /image/2/0 -> 200 image/png 73 nosniff Content-Disposition: inline; filename="1.png" body=73B
GET /image/2/1 -> 404 text/html; charset=utf-8 Content-Length=277 body=277B
GET /image/2/2 -> 404 text/html; charset=utf-8 Content-Length=271 body=271B
GET /image/1/2 -> 404 text/html; charset=utf-8 Content-Length=241 body=241B    ← 範囲外
GET /image/1/-1 -> 404 text/html; charset=utf-8 Content-Length=241 body=241B   ← 負数
```

後始末: 待受 PID（19880）を `taskkill /F`。`netstat` で 17941 に LISTEN が無いこと（TIME_WAIT のみ）と
`manga-viewer.exe` プロセスが無いことを確認。`tests/.tmp/p0-4-live/` は削除済み。

## 未対応・次へ

- 実画像（実在の漫画ファイル）では未検証。本タスクは合成データのみ。
- ピクセルへの復号は引き続きブラウザ側。サーバ側の復号が必要になったら `image` crate を本依存にする判断が要る。
