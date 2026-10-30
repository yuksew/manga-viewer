# MV-p0-4 監査報告 — 画像検証 imgcheck（Claude）

対象: branch `qwen-p0`、commit **26b6b8c**（HEAD と一致）。作業者の申告は使わず、すべて再測した。
漫画ファイル（`D:\books`・`D:\torrent`）には触れていない。合成データは `tests/.tmp/p0-4-audit/`（gitignore 済み）。

## 判定: **条件つきで使える**

HEAD の挙動に誤りは見つからなかった。serve 実走は 26 経路すべて期待どおりで、2 冊の for ループ trap も無い。
条件を付けるのは、**テストが imgcheck の境界・アンカー条件の一部を固定していない**ため。
変異 17 本のうち 7 本が生き残った。うち 5 本は、人工プローブで HEAD は通り変異体は落ちることを確認した（実害のある穴）。
残り 2 本は等価変異。現状は検出できているが、将来の回帰を止める網が薄い。
デコードしないので「ヘッダと終端は正しいが中身だけ壊れた画像」は通る。これは実装メモにも書かれた既知の限界。

## 1. 4 門（独立再測）

| 門 | 結果 |
|---|---|
| `cargo fmt --check` | 0 |
| `cargo clippy -q --all-targets` | 0（警告なし） |
| `cargo build` | 0 |
| `cargo test` | 全通過。**47 本** = lib 10 + image_check 6 + index_db 16 + scan_book 7 + server 8（main・doc は 0）。申告と一致 |

## 2. imgcheck の変異再実施

ハーネスは `tests/.tmp/p0-4-audit/mut.py`。変異を 1 本ずつ適用して `cargo test` を走らせ、必ず `git checkout` で戻す。
作業ツリーは CRLF なので、複数行パターンは CRLF に直して当てた（最初の回はこれで 2 本が不発になり、直して再実施）。

**imgcheck.rs 17 本中、10 本撃墜・7 本生存。** 加えて server.rs の 2 本は両方撃墜（合計 19 本中 12 落ち）。

| # | 変異 | 結果（落ちたテスト） |
|---|---|---|
| M01 | PNG: IEND 無しを Ok に | 落ち（truncation） |
| M02 | PNG: IEND 後ろのごみを許容 | 落ち（garbage） |
| M03 | JPEG: EOI チェック削除 | 落ち 2 本（truncation, per_page） |
| M04 | GIF: 末尾 `3B` チェック削除 | 落ち 2 本（truncation, per_page） |
| M05 | WebP: `RIFF` 検証削除 | **生存** |
| M06 | PNG magic を先頭以外にも一致 | **生存（等価）** |
| M07 | JPEG magic を先頭以外にも一致 | **生存** |
| M08 | PNG 境界 `len<33` → `<32` | **生存（等価）** |
| M09 | GIF 境界 `len<14` → `<13` | **生存** |
| M10 | WebP RIFF サイズ境界 +8 → +7 | 落ち（truncation） |
| M11 | WebP チャンク境界 +20 → +19 | **生存** |
| M12 | PNG 寸法 0 チェック削除 | 落ち（garbage） |
| M13 | JPEG 寸法 0 チェック削除 | 落ち（garbage） |
| M14 | JPEG 0 埋め許容を撤去 | 落ち（garbage） |
| M15 | GIF 寸法 0 チェック削除 | 落ち（garbage） |
| M16 | WebP チャンク種別チェック削除 | 落ち（garbage） |
| M17 | PNG IHDR 長さ 13 チェック削除 | **生存** |
| M18 | server: 検証失敗を握りつぶす | 落ち（per_page） |
| M19 | server: Content-Type を拡張子へ戻す | 落ち（content_type_follows_magic） |

### 生存の理由

等価変異（観測できる差が無い）:
- **M06**: magic が先頭に無いと、`png()` が IHDR を見つけられず結局 `Err`。理由文が変わるだけで結果は同じ。
- **M08**: `len=32` でも IHDR チャンクの終端 33 が実長を超え、後続の走査で `Err`。冗長なガード。

非等価（HEAD は正しく、変異体を落とすプローブを書いて確認した。プローブは commit していない）:
- **M05**: 先頭が `RIFX` の WebP 形が通る。RIFF 検証を固定するテストが無い。
- **M07**: 本体に `FF D8` を含む GIF が JPEG と誤判定され、**正常な GIF が 404 になる**。
  実害に近いのはこれ。WebP や GIF は EXIF・サムネイルに JPEG を内包し得る。magic を先頭で固定するテストが無い。
- **M09**: 13 バイトで末尾が `3B` の GIF が通る。切り詰め全長テストは最小 GIF の 13 バイト接頭辞の末尾が `00` なので見逃す。データ依存の穴。
- **M11**: WebP のチャンク長が RIFF を 1 バイトだけ超える場合が通る。
- **M17**: IHDR の長さが 13 以外の PNG が通る。

推奨テスト追加（未実施。監査は docs のみの方針）:
1. 本体に `FF D8` を含む GIF/WebP が正しく判定されるケース
2. `RIFX` 始まりの WebP が拒否されるケース
3. WebP チャンク長 `riff-11` のケース
4. IHDR 長さ 14 の PNG のケース
5. 13 バイトで末尾 `3B` の GIF のケース

## 3. serve 実走（port 17951）

合成 cbz 2 冊（`tests/.tmp/p0-4-audit/live/books/`、build.py で生成）。
`cargo run -q -- index … --db …` の結果は `books=2 pages=13 broken=0`。
続いて `serve --port 17951 --root …` を起動し、python の http.client で全頁を `/page` と `/image` の両方で取得した。
期待値との自動照合（ステータス・Content-Type・本体バイト一致・`nosniff`・Content-Length・Content-Disposition の有無と名前）の結果は **不一致 0**。

```
(src = 元ファイルのバイト数。200 の本体バイト数は src と一致、404 本体は 271〜274B の HTML エラー頁)
a[0] a0.png  src= 61B  IEND 欠落          page=404 text/html  image=404 text/html   (壊れた画像です: PNG に IEND がありません)
a[1] a1.jpg  src= 29B  正常 JPEG          page=200 image/jpeg 29B            image=200 image/jpeg 29B  inline; filename="2.jpg"
a[2] a2.gif  src= 35B  正常 GIF           page=200 image/gif  35B            image=200 image/gif  35B  inline; filename="3.gif"
a[3] a3.webp src= 28B  正常 WebP          page=200 image/webp 28B            image=200 image/webp 28B  inline; filename="4.webp"
a[4] a4.png  src= 29B  拡張子 png/中身 jpeg  page=200 image/jpeg 29B          image=200 image/jpeg 29B  inline; filename="5.jpg"
a[5] a5.jpg  src= 73B  拡張子 jpg/中身 png   page=200 image/png  73B          image=200 image/png  73B  inline; filename="6.png"
a[6] a6.jpg  src= 27B  EOI 欠落           page=404 text/html  image=404 text/html   (壊れた画像です: JPEG に EOI がありません)
b[0] b0.png  src= 73B  正常 PNG（python で生成）  page=200 image/png 73B     image=200 image/png 73B  inline; filename="1.png"
b[1] b1.png  src= 20B  20B 切り           page=404 text/html  image=404 text/html   (PNG の寸法が不正です)
b[2] b2.png  src= 73B  正常 PNG           page=200 image/png 73B             image=200 image/png 73B  inline; filename="3.png"
b[3] b3.jpg  src= 29B  magic あり・SOF 寸法 0  page=404 text/html  image=404 text/html (JPEG の寸法が不正です)
b[4] b4.png  src= 47B  PNG 署名＋ごみ      page=404 text/html  image=404 text/html   (PNG に IHDR がありません)
b[5] b5.gif  src= 35B  正常 GIF           page=200 image/gif  35B            image=200 image/gif  35B  inline; filename="6.gif"
edge /page/1/99, /image/2/99, /image/999/0, /image/1/-1, /image/x/0, /page/1/1/2 → すべて 404
book b p=1 → 200、<img src="/page/2/1"> のまま（HTML 側は壊れた頁でも img を出す）
```

- **Content-Type の中部（magic）判定の実証**: `a4.png`（拡張子 png・中身 jpeg）は `image/jpeg`、
  `a5.jpg`（拡張子 jpg・中身 png）は `image/png` が返った。`/image` のファイル名拡張子も中身側（`5.jpg`、`6.png`）。
- 200 は全部 `X-Content-Type-Options: nosniff`。Content-Disposition は `/image` だけに付き、`/page` には無い。
- 日本語のエラー文はコンソールの文字コードの都合で文字化けして記録された。上のログの括弧内は、ソース（`src/imgcheck.rs`）の理由文に対応づけた記載。
- 画像は合成の最小品（WebP はダミーペイロード）。実際の画像ファイルでのデコード可否は見ていない（imgcheck はデコードしない）。
- **サーバ残骸ゼロ**: 検証後に `taskkill` で止め、`tasklist` で manga-viewer.exe 0 件、port 17951 への接続は拒否（10061）を確認した。

## 4. for ループ trap 検証

serve 実走の 2 冊が不連続な壊れ位置を持つ。
- 冊 a（7 頁）: 壊れ位置は **0 番目と最終（6）**。1〜5 は 200。
- 冊 b（6 頁）: 壊れ位置は **1・3・4（中間）**。0・2・5 は 200。

どの頁も `/page`・`/image` の両方で個別に正しく判定された。冊をまたいでも判定が漏れない（冊 a の頁 6 の 404 が冊 b に影響しない）。
テスト側の `per_page_verdicts_across_two_books`（2 冊・計 8 頁）も同じ観点で、M03・M04・M18 を撃墜している。

## 作業ツリー

変異適用後はすべて `git checkout` で原状復帰。プローブ用の `tests/zz_probe.rs` は削除済み。
最終の `git status` は clean（監査成果物は gitignore 済みの `tests/.tmp/` と、この docs のみ）。
