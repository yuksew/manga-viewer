---
name: screenshot
description: manga-viewer の実窓を撮ってユーザーに見せる（モバイルでも見られるよう Orca のタブで開く）。「キャプチャ見せて」「スクショ撮って」「画面を見せて」や、表示まわりの変更を目で確かめたいときに使う。
---

# manga-viewer の画面を撮って見せる

## 要点

- OS の画面取り込み（CopyFromScreen・PrintWindow）では、egui(glow) の窓の中身が**白く写る**ので使わない。
- アプリには開発用のフックがある: `MV_SCREENSHOT=<フォルダ>` を付けて起動すると、**F12** でアプリ自身の描画結果をそのフォルダに PNG で保存する（stderr に `screenshot` 行が出る。`src/view.rs` の `save_screenshot_on_f12`）。
- 画面全体は撮らない。ユーザーの他の窓（私的な内容）が写る。
- ユーザーに見せるときは、ワークツリー内の `target/screenshots/`（git 管理外）に置いて `orca file open` で開く。Orca のタブとしてモバイルにも出る。ワークツリーの外のパスは開けない。

## 手順

`capture.ps1` がまとめてやる: ビルド → 起動 → 窓を前面にしてキーを順に送る → `shot` のたびに F12 で撮影 → 撮った順に左から 1 枚へ並べる → `target/screenshots/<Name>.png` に保存 → Orca で開く → 窓を閉じる。

```powershell
# PowerShell ツールで。-Book を省くと 5 頁のサンプル冊（COVER, 2〜5）を作って使う
& .\.claude\skills\screenshot\capture.ps1 -Keys f,shot,'{RIGHT}',shot -Name spread
& .\.claude\skills\screenshot\capture.ps1 -Book 'D:\books\x.zip' -Keys shot -Name page1
```

- PowerShell ツールから `&` で呼ぶ。`powershell -File` 経由だと `-Keys a,b` が配列にならず 1 つの文字列として打ち込まれる。
- `-Keys` は SendKeys の文字列（`f`、`{RIGHT}`、`{LEFT}`、`{PGDN}`、`{ESC}` など）。`shot` は撮影。
- `down:<x>,<y>` / `up:<x>,<y>` はその位置で左ボタンを押す / 離す（ドラッグの途中を撮るなら `down:…`, `mouse:…`, `shot`, `up:…`）。
- `dblclick:<x>,<y>` は同じ位置でダブルクリックする（例: 頁の上で全画面にするなら `dblclick:0.5,0.5`）。
- `mouse:<x>,<y>` は窓の中身の割合位置（0〜1）へカーソルを動かす。例: 下端の頁スライダーを出すなら `mouse:0.5,0.97`。ユーザーの実カーソルを動かすので、撮影中に一言断っておく。
- 冊を開く直後の待ちは `-WaitMs`（既定 3000）、キー間は `-StepMs`（既定 800）。重い冊なら延ばす。
- 一覧画面など `view --path` 以外の起動が要るときは、スクリプトの `Start-Process` の引数を参考に同じ流れを組む（`MV_SCREENSHOT` を付けて起動し、F12 を送る）。

- キーは前面の窓へ送られる。スクリプトは毎回ビューアが前面かを確かめ、前面にできなければ止める（他のアプリへ打ち込まない）。止まったら、ユーザーが別の窓を操作中かもしれないので、少し待ってからやり直す。

## 撮ったあと

- `Read` で `target/screenshots/<Name>.png` を見て、意図した画面か自分で確かめてから見せる。
- ユーザーには何を撮ったか（どの操作の後か、並び順）を一言添える。
