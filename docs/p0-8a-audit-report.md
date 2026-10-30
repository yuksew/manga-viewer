# p0-8b 監査報告: p0-8a 破損自動修復の変異テスト

対象: commit `a1be45c`（src/decode.rs の多段フォールバック、view 注記、serve `X-MV-Repaired`）。
方法: 1 変異ずつ src に注入 → `cargo test --target-dir target/alt --no-fail-fast`（全テスト）→ 元へ復元。
実データ（D:\books）は未使用。変異は終了後すべて戻し、`src/` は `a1be45c` と差分なし。

## 判定: **条件付き合格**

- 変異 31 件: 初回で **殺 21 / 生存 9 / コンパイル不能 1（M13b）**。生存 9 の内訳は
  無効（変異が no-op）1（M1b）、等価変異 6（M4c, M7, M7b, M8, M10, M12）、実質の穴 2（M3c, M17）。
- M3c は注記文言を検査するテストを追加して殺した（修復済み）。**最終の生存は実質 M17 の 1 本**
  （順序を固定するテストが無い。F1 と表裏）。
- 実装の欠陥（所見）は **F1（軽微・表示に影響なし）1 件**。

## 変異一覧

| # | 変異 | 結果 | 殺したテスト / 理由 |
|---|---|---|---|
| M1 | strict 成功でも `Truncated` 印 | 殺 | intact_images_have_no_repair_mark ほか 3 本 |
| M1b | strict 成功後に候補を計算するだけ | 無効 | 副作用なしの no-op（変異になっていない） |
| M2 | 候補数 16→1 | 殺 | png_garbage_containing_iend_… |
| M2b | 16→0 | 殺 | 5 本 |
| M2c | 16→2 | 殺 | png_garbage_containing_iend_… |
| M3 | `as_str` Truncated を trailing-garbage に | 殺 | serve_marks_repaired_pages_with_header |
| M3b | 逆方向に潰す | 殺 | 同上 |
| M3c | `note` の Truncated 文言を TrailingGarbage と同じに | **生存→殺** | 既存テストは「修復して表示」の有無しか見ていなかった。`view_note_text_matches_repair_kind` を追加して殺した |
| M4 | view 注記を常に出さない | 殺 | view_shows_repair_note_only_on_repaired_page |
| M4b | 注記描画を消す | 殺 | 同上 |
| M4c | Err 時に `note` を消さない | 生存（等価） | 注記の描画は texture ありの分岐内で、Err 時は texture=None なので描かれない。ただし頁送り後に stale が出ないのは texture 更新側で担保 |
| M5 | Truncated 救済を終端切りより先に | 殺 | 5 本（TrailingGarbage が Truncated に化ける） |
| M6 | Truncated 救済を strict に置換（=救済なし） | 殺 | jpeg_cut_tail_…, serve…, per_page_verdicts_… |
| M7 | sniff JPEG を `FF` のみに緩める | 生存（等価） | 緩めた先でも復号器が拒否する。SOI 破損は結局 Err |
| M7b | sniff 不一致を常に JPEG 扱い | 生存（等価） | 同上。非画像は JPEG 復号で必ず失敗し `strict_err` を返す |
| M8 | `end < len` を `<=` | 生存（等価） | 全長で切り直しても strict と同じ結果で、候補が 1 つ無駄になるだけ |
| M8b | 全長フィルタ除去 | 殺 | truncated_images_are_errors_not_panics |
| M9 | JPEG の切り位置 +2→+1 | 殺 | 3 本 |
| M9b | +2→+3 | 殺 | 3 本 |
| M9c | PNG +8→+4 | 殺 | 2 本 |
| M9d | PNG +8→+12 | 殺 | 2 本 |
| M10 | TooLarge の修復除外ガード除去 | 生存（等価） | 修復経路でも `decode_raw` が同じ TooLarge を返す。無駄な復号試行が増えるだけの性能差 |
| M11 | 失敗時のエラーを strict でなく別物に | 殺 | unrepairable_error_is_the_strict_decode_error |
| M12 | sniff None でも続行 | 生存（等価） | M7b と同じ |
| M13 | serve の `truncate(len)` 除去 | 殺 | serve_marks_repaired_pages_with_header |
| M13b | Truncated にも truncate 適用 | 無効 | パターンの束縛不一致でコンパイル不能 |
| M14 | ヘッダを付けない | 殺 | 同上 |
| M15 | 常に `X-MV-Repaired` を付ける | 殺 | 同上（無傷頁で None を検査） |
| M16 | `TrailingGarbage.len` を +1 | 殺 | 4 本 |
| M17 | 候補の並びを後ろから→前から | **生存** | どのテストも「後ろから」を前提にしていない。下記 F1 の通り前からの方が結果が良い |
| M18 | マーカー一致 filter を常に真 | 殺 | 3 本 |

カード指定の 5 候補への対応: 「strict 成功で修復経路」=M1/M15 殺、「候補数」=M2 系殺、
「TrailingGarbage/Truncated 区別」=M3/M3b/M5 殺（M3c は文言側で追加テストにより殺）、
「修復成功でも view 注記なし」=M4/M4b 殺、「imgcheck 検査位置・先頭側破損」=M6/M7/M11/M12 のうち
M7/M7b/M12 は生存だが等価（先頭側破損は復号器が拒否し `strict_err` が返る）。

## 所見

- **F1（軽微・実装）** 候補を後ろの `FFD9`/`IEND` から試すため、ゴミ自体に終端バイト列が含まれると、
  切り位置が真の終端より後ろになる（例: 1535 バイトのはずが 1561）。画素は正しく、view の表示には影響しない。
  serve が返す本体にゴミの一部が残る。再現は `jpeg_garbage_containing_eoi_bytes_is_still_cut_at_true_end`
  （`#[ignore]`）。M17（前から試す）にすると同テストは通る。ただし前から試すと、EXIF サムネイル内の
  `FFD9` で先に切る候補を余計に試す副作用があるので、修正方針は p0 の判断に委ねる（本カードでは実装を触らない）。
- F2（想定したが欠陥ではなかった）: ゴミに `FFD9` が 16 個超あっても、後ろ 16 個のどれで切っても
  strict 復号が通るため `TrailingGarbage` のまま。候補上限 16 が誤表示を生む経路は見つからなかった。
- 生存変異は M17 以外すべて等価。M17 は「順序を固定するテストが無い」ことを示す（F1 修正時に順序のテストが要る）。
- 作業ツリーの前提: 監査開始時に `tests/repair.rs` に未コミットの追記があり、`b"junkÿÙjunk"`（バイト列リテラルに
  非 ASCII）でコンパイルできなかった。`b"junk\xFF\xD9junk"` に直して採用した。

## 追加したテスト（tests/repair.rs）

| テスト | 目的 |
|---|---|
| view_note_text_matches_repair_kind | M3c を殺す（注記文言が修復種別と対応） |
| png_garbage_containing_iend_is_still_cut_at_true_end | PNG の IEND を含むゴミ（M2/M2c/M9c/M9d/M16 を殺す） |
| unrepairable_error_is_the_strict_decode_error | M11 を殺す |
| jpeg_garbage_containing_eoi_bytes_still_decodes_same_pixels | FFD9 入りゴミでも画素一致 |
| jpeg_garbage_containing_eoi_bytes_is_still_cut_at_true_end (`#[ignore]`) | F1 の再現。修正時に ignore を外す |

テスト本数（実測、`target/alt`）: **83 pass / 0 fail / 1 ignored**（p0-8a の 79 から +4、+ignored 1）。
