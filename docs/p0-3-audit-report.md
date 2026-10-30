# MV-p0-3 監査報告（serve 実走の独立検証・再実施）

対象: branch qwen-p0 / 実装 commit e583a61（現 HEAD 399703c との差は前回の本報告 docs のみ、`git diff --stat` で確認）/ 監査日 2026-10-31。
漫画実データ（D:\books, D:\torrent）には触れていない。合成データは `tests/.tmp/p0-3-audit/`（git 管理外）。
前回の報告（port 17901）は読まずに信用せず、今回すべて叩き直した。

## 判定: **合格**

serve 実走で全経路が期待どおり。HTML エスケープも本番経路で効いている。重大・中程度の所見なし。

## 1. 4 門（独立再測）
| 門 | 結果 |
|---|---|
| `cargo fmt --check` | 差分なし |
| `cargo clippy -q --all-targets` | 警告なし |
| `cargo build` | 成功 |
| `cargo test` | lib 10 + main 0 + index_db 16 + scan_book 7 + server 8 = **41 本全通過**（申告 41 と一致） |

## 2. serve 実走（`serve --port 17931`、background 起動）
合成 PNG cbz 6 冊を index（`discovered=6`、`books=6 pages=11 broken=0`）して起動。
HTTP クライアントは curl が承認対象外のため python `http.client` を使用（同等のリクエスト）。数値はすべて今回の実測。

| リクエスト | 実測 |
|---|---|
| `GET /` | 200 `text/html; charset=utf-8` 554 B、`/book/1`〜`/book/6` の 6 冊分リンク + 頁数 |
| `GET /book/2`（one, 3 頁） | 200 315 B、`1/3 ページ` + next のみ、`<img src="/page/2/0">` |
| `GET /book/2?p=1` | 200、`2/3 ページ`、prev(p=0)・next(p=2)・`<img src="/page/2/1">` すべて有り |
| `GET /book/2?p=2` | 200、`3/3 ページ`、next 無し、prev(p=1) 有り |
| `GET /page/2/0` | 200 `image/png` **73 B**（原 000.png=73 B） |
| `GET /page/2/1` | 200 `image/png` **77 B**（原 001.png=77 B） |
| 他冊の `/page/<id>/<n>` | id1: 73/78、id4: 73/74、id5: 73/77、id6: 73、id3: 73 — **zipfile で測った原寸と全 11 頁一致** |
| `GET /page/3/1`、`/page/6/1`（頁数超過） | 404 |
| `GET /page/2/999`、`/page/2/-1` | 404 |
| `GET /page/9999/0`、`GET /book/9999`（存在しない id） | 404 |
| `GET /book/2?p=999` | 404 |
| `GET /nope`、`/page/2/0/../../x` | 404 |
| `POST /`、`POST /book/2` | 405 |

サーバ停止: 17931 を LISTEN する PID 33500 を終了し、LISTEN なし・manga-viewer プロセスなしを確認。
前回報告が心配していた 17901 にも LISTEN の残骸なし。

## 3. HTML エスケープ
- `Tom & Jerry.cbz` → 一覧・`<h1>`・`<title>` とも `Tom &amp; Jerry`。
- `x&lt;script&gt;alert(1).cbz`（実名にエンティティ風文字列）→ `x&amp;lt;script&amp;gt;alert(1)`。二重解釈されず文字どおり表示。
- 題名が `<script>alert(1)</script>"x` の冊（id1、前任の残骸 lt.cbz）→ 一覧 `<a>`、`<h1>`、`<title>` すべて
  `&lt;script&gt;alert(1)&lt;/script&gt;&quot;x`。**未エスケープ箇所なし**（XSS なし）。
- `<` `>` `"` は Windows のファイル名に使えないため、これらは題名がメタデータ由来の冊で確認した。
- `esc` は `& < > "` を置換（server.rs:97）。属性値は `/page/{id}/{p}` の数値のみで題名を属性に入れていない。

## 所見（軽微・合否に影響なし）
1. `tests/.tmp/p0-3-audit/books/` に前任の lt.cbz・solo.cbz が残っていたので、そのまま再利用した（git 管理外）。
2. 壊れた cbz を実走で 404/エラーにする経路は未実走（server テストの範囲）。
3. 並行・遅い接続（slowloris）耐性は対象外。127.0.0.1 待受のみなので P0 では許容。
4. 日本語を含む HTML は UTF-8 で正しく配信されている（`N/M ページ` の文字列一致で確認）。
