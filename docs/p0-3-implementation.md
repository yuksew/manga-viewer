# p0-3 実装メモ — ローカル HTTP サーバ

`manga-viewer serve [--port N] --db PATH [--root R]`（既定ポート 7878、`0` で空きポート）。
`127.0.0.1` だけに bind する。実装は `src/server.rs`、試験は `tests/server.rs`。

## ルート

| 要求 | 応答 |
|---|---|
| `GET /` | 冊の一覧 HTML（索引の自然順）。リンクは `/book/<id>` |
| `GET /book/<id>[?p=N]` | N 頁目（既定 0）の `<img>` + prev/next + 「N/M ページ」 |
| `GET /page/<id>/<n>` | 画像バイト。Content-Type は拡張子から |
| 上記以外 | 404 |
| GET 以外 | 405（`Allow: GET`）。HEAD も 405 |
| 請求行が壊れている | 400 |

id・n・p は数字だけを受ける（`-1`、`+1`、空、`usize` 溢れは 404）。
`/book/<id>` の頁指定は設計書に無かったので `?p=N`（0 始まり）にした。

## 設計判断

- **依存は追加しない**（Cargo.toml は無変更）。std の `TcpListener` + 接続ごとに 1 スレッドで足りる。
  扱うのは GET 3 種だけで、keep-alive・chunked・TLS は要らない。hyper は tokio 一式を引き込み、
  ビルド時間と依存が膨らむ割に得るものが無い。応答は `Connection: close` の HTTP/1.0 流儀。
- **ハンドラは I/O と分離**: `handle(&Index, Option<&Path>, &str) -> Response`。請求文字列を入れて
  応答を得るので、ソケット無しで単体試験できる（冊の読み出しだけはする）。
- **パスは URL から組まない**: 画像は「索引の行（path, kind）→ `Book::open` → 頁一覧の n 番目」で決める。
  URL が持つのは数字の id と n だけなので、パス・トラバーサルの余地が無い。
- **接続ごとに `Index::open`**: rusqlite の `Connection` は共有できないので、スレッドごとに開く。
  SQLite の open は軽く、ローカル 1 人用なので許容。冊（zip の中央ディレクトリ）も要求ごとに開き直す。
  キャッシュは p0-4 以降（性能が問題になってから）。
- **`--root`**: 与えるとその下の冊だけを配る（`books_under`）。無ければ索引の全部。
- **HTML は `&<>"` をエスケープ**（題名がファイル名由来のため）。
- ヘッダ読みは 8 KiB 上限・読み取り 5 秒タイムアウト。

## エラー処理

| 状況 | 応答 |
|---|---|
| id が索引に無い／`--root` の外 | 404 |
| 冊が `broken`（索引に記録済み） | 404 + 理由。一覧には「壊れています」と出る |
| 冊が開けない（索引後に消えた等） | 404 + 理由 |
| 頁が範囲外 | 404 |
| 索引 DB を開けない・SQL 失敗 | 500 |

## decode の扱い（要確認事項）

依頼文の「p0-1 の decode」は、現状 `Book::read_page_bytes`（zip/フォルダから生バイトを取り出す）が
それに当たる。ピクセルへの復号は実装されておらず、サーバは**バイトをそのままブラウザへ渡す**
（復号はブラウザ側）。

索引が拾う頁は `book.rs` の `IMAGE_EXTS`（png/jpg/jpeg/webp/gif）だけで、サーバの Content-Type 表と
一致している。つまり「索引が拾うが配れない形式」は現状ありえない。念のため表に無い拡張子は
`/page` で 404「未対応」、`/book` で「未対応」の文言にしてある（将来 `IMAGE_EXTS` が増えたときの保険）。
このため索引側のフィルタは不要と判断した。

## 試験

41 本（before 33 → +8、すべて `tests/server.rs`）。Content-Type 判定、一覧の順、頁めくり境界
（p=0 / M-1 / M / -1）、`/page` の範囲（0, M-1, M, -1, 溢れ）、各種 404・405・400、`--root` 絞り込み、
HTML エスケープ、`127.0.0.1:0` に bind した実ソケットで HTTP/1.0 GET（200 のヘッダとバイト一致・404）。

## 実測ログ

合成 cbz 3 冊（`tests/.tmp/p0-3/books/`、alpha=3 頁・beta=2 頁・gamma=5 頁の PNG）を `index` →
`serve --port 17878` を background で起動 → curl（`GET path -> status content-type bytes`）:

```
GET / -> 200 text/html; charset=utf-8 339
GET /book/1 -> 200 text/html; charset=utf-8 319
GET /book/1?p=2 -> 200 text/html; charset=utf-8 319
GET /book/1?p=3 -> 404 text/html; charset=utf-8 241
GET /page/1/0 -> 200 image/png 514
GET /page/1/2 -> 200 image/png 762
GET /page/1/3 -> 404 text/html; charset=utf-8 241
GET /page/2/1 -> 200 image/png 830
GET /page/3/4 -> 200 image/png 1000
GET /book/99 -> 404 text/html; charset=utf-8 256
GET /nope -> 404 text/html; charset=utf-8 241
POST / -> 405
```

zip 内のエントリ実寸と一致: alpha/001.png=514、alpha/003.png=762、beta/002.png=830、gamma/005.png=1000。
`/book/1?p=1` の HTML は `prev`（p=0）・`2/3 ページ`・`next`（p=2）・`<img src="/page/1/1">`。
