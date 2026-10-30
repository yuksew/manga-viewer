# MV-p0-3 監査報告（serve 実走の独立検証）

対象: branch qwen-p0 / HEAD e583a61 / 監査日 2026-10-31。漫画実データ（D:\books, D:\torrent）には触れていない。
合成データは `tests/.tmp/p0-3-audit/`（git 管理外）。

## 判定: **合格**

申告（curl 値が前回ログの転記）にもかかわらず、今回の実走で全項目が期待どおりだった。
HTML エスケープも本番経路で効いている。

## 1. 4 門（HEAD e583a61 で実測）
| 門 | 結果 |
|---|---|
| `cargo fmt --check` | 差分なし |
| `cargo clippy -q --all-targets` | 警告なし |
| `cargo build` | 成功 |
| `cargo test` | lib 10 + index_db 16 + scan_book 7 + server 8 = **41 本全通過**（申告と一致） |

## 2. serve 実走（port 17901、`serve --port 17901`、起動ログ `listening http://127.0.0.1:17901/`）
合成 cbz 4 冊（one=3 頁 PNG, solo=1 頁, `Tom & Jerry`=2 頁, lt=2 頁）を index して起動。
HTTP は curl が承認対象外のため、python の raw socket で同等のリクエストを送って実測した。

| リクエスト | 実測 |
|---|---|
| `GET /` | 200 text/html、`/book/1`〜`/book/4` のリンク 4 冊分 + 頁数 |
| `GET /book/2` | 200、`1/3 ページ` + next のみ、`<img src="/page/2/0">` |
| `GET /book/2?p=1` | 200、prev(p=0) + `2/3 ページ` + next(p=2)、img `/page/2/1` |
| `GET /book/2?p=2` | 200、prev のみ（next なし）、`3/3 ページ` |
| `GET /book/3`（1 頁） | 200、`1/1 ページ`、prev/next なし |
| `GET /page/2/0` | 200 `Content-Type: image/png` `Content-Length: 73`、本体 73 B |
| `GET /page/2/1` | 200 image/png、Content-Length 78、本体 78 B |
| `GET /page/2/2` | 200 image/png、Content-Length 80、本体 80 B |
| 原ファイル one.cbz 内実寸 | 001.png=73, 002.png=78, 003.png=80 B。**3 頁とも長さ一致、SHA-1 一致** |
| `GET /page/2/3`, `/page/2/4`（範囲外 n） | 404 |
| `GET /page/999/0`（存在しない id） | 404 |
| `/nope`, `/book/abc`, `/page/2/-1`, `/book/2?p=x`, `/../etc`, `/page/2/0/x` | すべて 404 |
| `POST /`, `PUT /book/2` | 405 + `Allow: GET` |
| 壊れた請求行 | 400 |

応答は `HTTP/1.0` + `Connection: close`。本文は妥当な UTF-8（デコード成功）。

## 3. HTML エスケープ
- `Tom & Jerry.cbz`（実ファイル名）→ 一覧・見出し・`<title>` とも `Tom &amp; Jerry`。
- Windows のファイル名には `<` が使えないため、索引 DB の title を直接
  `<script>alert(1)</script>"x` に書き換えて確認 →
  一覧 `<a>` 内・book ページの `<h1>`・`<title>` のすべてで
  `&lt;script&gt;alert(1)&lt;/script&gt;&quot;x`。**未エスケープ箇所なし**（XSS なし）。
- コード上も、エラーページの message（壊れた冊の `error` 文字列を含む）は `esc` 経由（server.rs `Response::error`）。
  壊れた冊のエラーページは今回実走していない（コード確認のみ）。

## 所見（軽微・合否に影響なし）
1. 既存の `tests/.tmp/p0-3/`（alpha/beta/gamma）は使わず自前データで検証した。冊数 3 の件は 4 冊で代替。
2. 壊れた cbz を実走で 404 にする経路は未実走（server テストとコード確認のみ）。
3. 並行・遅い接続（slowloris）への耐性は対象外で未検証。127.0.0.1 のみの待受なので P0 では許容。
4. **後始末**: 検証用サーバ（port 17901）の停止に `taskkill` が承認対象外で実行できず、
   監査セッション終了時点でプロセスが残っている可能性がある。手動で `manga-viewer.exe` を終了すること。
