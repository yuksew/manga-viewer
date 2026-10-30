# manga-viewer

zip / フォルダの漫画を、ブラウザなしの単独ウィンドウで読むビューア（Windows）。exe は 1 つで、外部ランタイムも要らない。

## 使い方（2 コマンドで完結）

```
manga-viewer index <root>                 # root 以下を走査して索引 DB に登録する
manga-viewer view --book <id> [--page N]  # 索引の冊を単独ウィンドウで開く
```

1. `index <root>` で冊を索引する（既定の root は `D:\books`）。索引 DB は既定で `%LOCALAPPDATA%\manga-viewer\index.db`。`--db <path>` か環境変数 `MANGA_VIEWER_DATA` で変えられる。
2. `list` で冊の id を確かめる（TSV: id / title / kind / page_count / status / path）。
3. `view --book <id>` で開く。`--book` を省略すると最初の壊れていない冊を開く。`--page N` は 0 始まりで、頁数以上なら最後の頁に丸める。
4. 操作は ←/→ で頁送り、Esc で終了。

ビルドは `cargo build --release`。成果物は `target/release/manga-viewer.exe` 1 つだけで、そのまま配れる。

## 壊れた画像

画像が壊れていても読み続けられるように、view は復号に失敗した頁を自動で修復する（終端マーカーで切る、末尾が欠けた分は部分復号する）。修復した頁には、画面に注記（「途中で切れて…」など）を出す。修復できない頁は「この頁は表示できません」と出し、ほかの頁はそのまま読める。

## serve はデバッグ専用

`serve` は 127.0.0.1 でブラウザから索引・頁・壊れ判定を目視確認するための開発用の窓口で、ユーザー向けの閲覧手段ではない。P0 の完了条件にも含めない。経緯と view との挙動差の決定は [docs/p0-6-design-options.md](docs/p0-6-design-options.md) の §6（MV-p0-7c）を参照。
