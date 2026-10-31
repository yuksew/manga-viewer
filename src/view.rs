//! 単独ウィンドウの閲覧（p0-6b）。`manga-viewer view` から呼ぶ。
//!
//! 1 冊を開いて 1 頁を窓に収めて表示し、→/PageDown/Space で次、←/PageUp で前、Esc で終了。
//! 復号は [`crate::decode`]（`imgcheck` を通ったバイトだけ）。壊れた頁は画像を出さずエラー文言を出す。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use eframe::egui;

use crate::book::Book;
use crate::decode::{self, Decoded};
use crate::drag::{Drag, DragInput};
use crate::index::{BookRow, Index, Status};
use crate::neighbor::{NavDir, resolve_neighbor};
use crate::scan::BookKind;

/// 窓が空のときに出すガイド文言。
pub const DROP_GUIDE: &str = "ここに zip またはフォルダをドロップ";

/// パスを直接開く（DB 不要）。フォルダなら画像フォルダ、`.zip`/`.cbz` なら ZIP として読む。
pub fn open_path(path: &Path) -> anyhow::Result<(String, Book)> {
    let meta = std::fs::metadata(path).with_context(|| path.display().to_string())?;
    let kind = if meta.is_dir() {
        BookKind::Folder
    } else if path.to_str().is_some_and(crate::book::is_zip_name) {
        BookKind::Zip
    } else {
        return Err(anyhow!(
            "zip（cbz）かフォルダではありません: {}",
            path.display()
        ));
    };
    let book = Book::open(path, kind).with_context(|| path.display().to_string())?;
    let title = path.file_stem().or_else(|| path.file_name()).map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    Ok((title, book))
}

/// 索引から冊を選んで開く。`id` が無ければ最初の壊れていない冊。
pub fn open_from_index(
    db: &Path,
    root: Option<&Path>,
    id: Option<i64>,
) -> anyhow::Result<(String, Book)> {
    let index = Index::open(db)?;
    let rows = match root {
        Some(root) => index.books_under(root)?,
        None => index.books()?,
    };
    let row = match id {
        Some(id) => rows
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| anyhow!("冊 {id} は索引にありません"))?,
        None => rows
            .into_iter()
            .find(|r| r.status != Status::Broken)
            .ok_or_else(|| anyhow!("開ける冊が索引にありません（先に index を実行）"))?,
    };
    if row.status == Status::Broken {
        let why = row.error.unwrap_or_default();
        return Err(anyhow!("壊れた冊です: {why}"));
    }
    let book = Book::open(&row.path, row.kind).with_context(|| row.path.display().to_string())?;
    Ok((row.title, book))
}

/// 頁 `i` のバイトを読み、検証つきで復号する。失敗は表示用の文言にする。
pub fn load_page(book: &Book, i: usize) -> Result<Decoded, String> {
    let bytes = book.read_page_bytes(i).map_err(|e| e.to_string())?;
    decode::decode_rgba(&bytes).map_err(|e| e.to_string())
}

/// `cur` から `delta` 頁動かした先（範囲外では動かない）。
#[must_use]
pub fn step(cur: usize, delta: isize, count: usize) -> usize {
    cur.saturating_add_signed(delta)
        .min(count.saturating_sub(1))
}

/// 見開きにする最小の窓幅（これより狭いと自動で単頁）。
pub const SPREAD_MIN_WIDTH: f32 = 700.0;

/// 見開きの希望と窓幅から、実際に見開きにするか。
#[must_use]
pub fn spread_active(want: bool, width: f32) -> bool {
    want && width >= SPREAD_MIN_WIDTH
}

/// 見開きの状態を窓内に出す文言（切替が効いたか・効かない理由を利用者に見せる）。
#[must_use]
pub fn mode_label(want: bool, width: f32, active: bool, page: usize, count: usize) -> String {
    if !want {
        "単頁（F で見開き）".to_string()
    } else if !active {
        format!("単頁（窓幅 {width:.0} が {SPREAD_MIN_WIDTH:.0} 未満のため見開きにできません）")
    } else if page + 1 >= count {
        "見開き（最終頁は 1 枚）".to_string()
    } else {
        "見開き（F で単頁）".to_string()
    }
}

/// 先読みする頁。今の表示の次と前を 1 送り分ずつ、次を先に（範囲内のみ）。
#[must_use]
pub fn prefetch_plan(page: usize, spread: bool, count: usize) -> Vec<usize> {
    let span = if spread { 2 } else { 1 };
    let next = (page + span..page + span * 3).filter(|&i| i < count);
    let prev = (page.saturating_sub(span * 2)..page).rev();
    next.chain(prev).collect()
}

/// キャッシュに残す頁の範囲（表示中の前後）。外は捨ててメモリを抑える。
#[must_use]
pub fn keep_range(page: usize, count: usize) -> std::ops::Range<usize> {
    page.saturating_sub(6)..(page + 8).min(count)
}

/// F（見開きトグル）が押されたか。IME 等でキーイベントが来ずテキストだけ来る場合も拾う。
fn spread_toggle_pressed(i: &egui::InputState) -> bool {
    i.key_pressed(egui::Key::F)
        || i.events.iter().any(
            |e| matches!(e, egui::Event::Text(t) if matches!(t.as_str(), "f" | "F" | "ｆ" | "Ｆ")),
        )
}

/// このフレームの入力から頁送りの向きと量を決める（次 +1 / 前 -1）。
///
/// キーが押されていればキーだけを数え、ホイールは見ない（二重計上しない）。
/// キーが無ければホイール 1 イベントにつき下 +1 / 上 -1。
fn nav_delta(i: &egui::InputState) -> isize {
    let next = i.key_pressed(egui::Key::ArrowRight)
        || i.key_pressed(egui::Key::PageDown)
        || i.key_pressed(egui::Key::Space);
    let prev = i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp);
    if next || prev {
        return isize::from(next) - isize::from(prev);
    }
    i.events
        .iter()
        .map(|e| match e {
            egui::Event::MouseWheel { delta, .. } if delta.y < 0.0 => 1,
            egui::Event::MouseWheel { delta, .. } if delta.y > 0.0 => -1,
            _ => 0,
        })
        .sum()
}

/// 頁送りの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    /// 頁が変わる（または変わらない）
    Page(usize),
    /// 端を越えようとした。次冊/前冊の確認を出す
    Prompt(NavDir),
}

/// 先頭側の頁 `page` から `delta` 回送る。見開きは 1 回 2 頁。
/// 見開きは右綴じ（右半分が `page`=偶数索引、左半分が `page+1`=奇数索引）なので、
/// 最終頁判定は右側の頁 `page`（偶数索引）ではなく見開きの末尾 `page+1`（左側）で行い、
/// 最初頁判定は `page == 0`（右側の頁）で行う。端の意味は単頁時と変わらない。
#[must_use]
pub fn plan_nav(page: usize, delta: isize, spread: bool, count: usize) -> Nav {
    let last = count.saturating_sub(1);
    let right = if spread { page + 1 } else { page };
    if delta > 0 && right >= last {
        Nav::Prompt(NavDir::Next)
    } else if delta < 0 && page == 0 {
        Nav::Prompt(NavDir::Prev)
    } else {
        let span = if spread { 2 } else { 1 };
        Nav::Page(step(page, delta * span, count))
    }
}

/// 索引から並び順の一覧を読む（開けなければ `None` でフォルダ順に落ちる）。
fn load_rows(db: &Path, root: Option<&Path>) -> Option<Vec<BookRow>> {
    let index = Index::open(db).ok()?;
    match root {
        Some(root) => index.books_under(root),
        None => index.books(),
    }
    .ok()
}

/// 復号済みの 1 頁分（テクスチャ化済み）。頁番号ごとにキャッシュし、F の切替や頁戻しで再復号しない。
#[derive(Default)]
struct Slot {
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    /// 修復して表示されているときの注記
    note: Option<&'static str>,
}

/// 裏スレッドで復号した結果（世代, 頁, 読み出し ms, 結果）。
type Loaded = (u64, usize, f64, Result<Decoded, String>);

pub struct ViewApp {
    title: String,
    /// 開いている冊。空の窓（ドロップ待ち）では `None`
    book: Option<Arc<Book>>,
    page: usize,
    /// 見開きの希望（窓が狭いと実際は単頁）
    spread: bool,
    /// 次冊/前冊を開くか確認中の向き。出ている間は他の送り入力を無視する
    prompt: Option<NavDir>,
    /// 候補が無かったなどの窓内通知
    notice: Option<&'static str>,
    /// 索引 DB と root（あれば次冊/前冊は索引順で探す）
    index: Option<(PathBuf, Option<PathBuf>)>,
    /// ドロップで開けなかったときの文言
    open_error: Option<String>,
    /// 頁番号 → 復号済みの頁
    cache: HashMap<usize, Slot>,
    /// 裏で復号中の頁
    inflight: HashSet<usize>,
    /// 冊を替えるたびに増やす。古い冊の先読み結果を捨てる
    generation: u64,
    sync_loads: usize,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
    /// 今表示している頁（左から）
    visible: Vec<usize>,
    /// 窓ドラッグ移動の状態
    drag: Drag,
    deadline: Option<Instant>,
}

impl ViewApp {
    /// `page` は範囲外なら最後の頁に丸める（[`run`] と同じ）。
    #[must_use]
    pub fn new(title: String, book: Book, page: usize, exit_after: Option<Duration>) -> Self {
        let page = step(page, 0, book.page_count());
        Self {
            title,
            book: Some(Arc::new(book)),
            page,
            deadline: exit_after.map(|d| Instant::now() + d),
            ..Self::empty()
        }
    }

    /// 冊のない空の窓（ドロップ待ち）。
    #[must_use]
    pub fn empty() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            title: String::new(),
            book: None,
            page: 0,
            spread: false,
            prompt: None,
            notice: None,
            index: None,
            open_error: None,
            cache: HashMap::new(),
            inflight: HashSet::new(),
            generation: 0,
            sync_loads: 0,
            tx,
            rx,
            visible: Vec::new(),
            drag: Drag::default(),
            deadline: None,
        }
    }

    /// 次冊/前冊を索引 DB の順で探すようにする（無ければ同フォルダの自然順）。
    #[must_use]
    pub fn with_index(mut self, db: PathBuf, root: Option<PathBuf>) -> Self {
        self.index = Some((db, root));
        self
    }

    /// 確認中の向き。
    #[must_use]
    pub fn prompt(&self) -> Option<NavDir> {
        self.prompt
    }

    /// 今の左頁（0 始まり）。
    #[must_use]
    pub fn page(&self) -> usize {
        self.page
    }

    /// 復号済みでキャッシュにある頁数。
    #[must_use]
    pub fn cached_pages(&self) -> usize {
        self.cache.len()
    }

    /// 表示のために同期で復号した回数（先読みが間に合わなかった数）。
    #[must_use]
    pub fn sync_loads(&self) -> usize {
        self.sync_loads
    }

    /// 開いている冊の表示名。
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// `path`（zip かフォルダ）を開いて今の冊と入れ替える。失敗なら今の表示は保ち、文言を出す。
    pub fn open_dropped(&mut self, path: &Path) {
        match open_path(path) {
            Ok((title, book)) => {
                self.title = title;
                self.book = Some(Arc::new(book));
                self.page = 0;
                self.prompt = None;
                self.notice = None;
                self.open_error = None;
                self.cache.clear();
                self.inflight.clear();
                self.visible.clear();
                self.generation += 1;
            }
            Err(err) => self.open_error = Some(format!("{err:#}")),
        }
    }

    /// 次冊/前冊の確認への答え。はいなら候補を解決して開く（無ければ通知を出して現在冊のまま）。
    pub fn answer_prompt(&mut self, yes: bool) {
        let Some(dir) = self.prompt.take() else {
            return;
        };
        if !yes {
            return;
        }
        let Some(book) = &self.book else { return };
        let cur = book.path().to_path_buf();
        let rows = self
            .index
            .as_ref()
            .and_then(|(db, root)| load_rows(db, root.as_deref()));
        match resolve_neighbor(&cur, dir, rows.as_deref()) {
            Some(path) => self.open_dropped(&path),
            None => {
                self.notice = Some(match dir {
                    NavDir::Next => "次の冊はありません",
                    NavDir::Prev => "前の冊はありません",
                });
            }
        }
    }

    /// 確認の表示とボタン。Enter=はい / Esc=いいえ。
    fn show_prompt(&mut self, ui: &mut egui::Ui, dir: NavDir, keys: bool) {
        let ctx = ui.ctx().clone();
        let mut answer = None;
        if !keys {
        } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            answer = Some(true);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
        ui.horizontal(|ui| {
            ui.label(match dir {
                NavDir::Next => "最後の頁です。次の冊を開きますか？（Enter=はい / Esc=いいえ）",
                NavDir::Prev => "最初の頁です。前の冊を開きますか？（Enter=はい / Esc=いいえ）",
            });
            if ui.button("はい").clicked() {
                answer = Some(true);
            }
            if ui.button("いいえ").clicked() {
                answer = Some(false);
            }
        });
        if let Some(yes) = answer {
            self.answer_prompt(yes);
        }
    }

    /// 1 フレーム分の描画（入力処理・頁読み込み・ラベル/画像/エラー文言）。
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_dropped(&path);
        }
        self.update_window_drag(ctx);
        if let Some(err) = &self.open_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
        if self.book.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.centered_and_justified(|ui| ui.label(DROP_GUIDE));
            return;
        }

        // 確認中は他の送り入力（キー/ホイール）を見ない。出した同じフレームの Enter/Esc も見ない
        let was_prompting = self.prompt.is_some();
        if !was_prompting {
            self.handle_input(ctx);
        }
        if let Some(dir) = self.prompt {
            self.show_prompt(ui, dir, was_prompting);
        }
        let Some(count) = self.book.as_deref().map(Book::page_count) else {
            return;
        };
        let width = ctx.content_rect().width();
        let spread = spread_active(self.spread, width);
        self.ensure_loaded(ctx, spread);

        let label = if spread && self.page + 1 < count {
            format!("{}-{}/{}", self.page + 1, self.page + 2, count)
        } else {
            format!("{}/{}", self.page + 1, count)
        };
        let mode = mode_label(self.spread, width, spread, self.page, count);
        ui.label(format!("{}  {label}  [{mode}]", self.title));
        if let Some(notice) = self.notice {
            ui.colored_label(egui::Color32::YELLOW, notice);
        }
        // 左右の領域を明示的に切り、各頁をその中へ描く（レイアウト任せにしない）
        let area = ui.available_rect_before_wrap();
        let w = area.width() / self.visible.len().max(1) as f32;
        let n = self.visible.len();
        for (k, i) in self.visible.iter().enumerate() {
            let Some(slot) = self.cache.get(i) else {
                continue;
            };
            // 右綴じ: 見開きは visible=[page, page+1] の page（1ページ目）を右半分、page+1 を左半分に置く
            let k = if n == 2 { 1 - k } else { k };
            let min = area.min + egui::vec2(w * k as f32, 0.0);
            let rect = egui::Rect::from_min_size(min, egui::vec2(w, area.height()));
            draw_slot(ui, rect, slot);
        }
    }

    /// 描画領域のどこでも押し下げ＋ドラッグで窓を動かす（状態遷移は [`crate::drag`]、優先順位もそこに明記）。
    /// 移動は OS 任せのドラッグ（`StartDrag`）で、閾値を超えた瞬間に 1 回だけ送る。
    fn update_window_drag(&mut self, ctx: &egui::Context) {
        let on_widget = ctx.egui_wants_pointer_input();
        let input = ctx.input(|i| DragInput {
            pressed: i.pointer.primary_pressed(),
            down: i.pointer.primary_down(),
            pos: i.pointer.interact_pos().map(|p| (p.x, p.y)),
            on_widget,
        });
        if self.drag.update(input) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    /// 確認中でないときのキー/ホイール入力（Esc 終了・F トグル・頁送り）。
    fn handle_input(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(spread_toggle_pressed) {
            self.spread = !self.spread;
        }
        // 窓移動中はキー/ホイールの頁送りを出さない
        let delta = if self.drag.is_moving() {
            0
        } else {
            ctx.input(nav_delta)
        };
        if delta == 0 {
            return;
        }
        let count = self.book.as_deref().map_or(0, Book::page_count);
        let spread = spread_active(self.spread, ctx.content_rect().width());
        self.notice = None;
        match plan_nav(self.page, delta, spread, count) {
            Nav::Page(p) => self.page = p,
            Nav::Prompt(dir) => self.prompt = Some(dir),
        }
    }

    /// 裏スレッドの完了分を取り込む。`wait` の頁が復号中なら届くまで待つ。
    fn drain(&mut self, ctx: &egui::Context, wait: Option<usize>) {
        loop {
            let msg = match wait.filter(|w| self.inflight.contains(w)) {
                Some(_) => self.rx.recv().ok(),
                None => self.rx.try_recv().ok(),
            };
            let Some((generation, i, read_ms, res)) = msg else {
                return;
            };
            if generation != self.generation {
                continue;
            }
            self.inflight.remove(&i);
            self.cache
                .insert(i, upload(ctx, i, read_ms, res, "prefetch"));
        }
    }

    /// 表示する頁を揃え（無ければ復号）、次/前を裏で先読みし、遠い頁を捨てる。
    /// キャッシュは頁番号キーなので、F の切替では既に読んだ頁を再復号しない。
    fn ensure_loaded(&mut self, ctx: &egui::Context, spread: bool) {
        let Some(book) = self.book.clone() else {
            return;
        };
        let count = book.page_count();
        let upto = if spread {
            (self.page + 1).min(count - 1)
        } else {
            self.page
        };
        self.visible = (self.page..=upto).collect();
        for i in self.visible.clone() {
            self.drain(ctx, Some(i));
            if !self.cache.contains_key(&i) {
                let (read_ms, res) = read_and_decode(&book, i);
                self.sync_loads += 1;
                self.cache.insert(i, upload(ctx, i, read_ms, res, "sync"));
            }
        }
        self.drain(ctx, None);
        for i in prefetch_plan(self.page, spread, count) {
            if self.cache.contains_key(&i) || !self.inflight.insert(i) {
                continue;
            }
            let (book, tx, ctx, generation) =
                (book.clone(), self.tx.clone(), ctx.clone(), self.generation);
            rayon::spawn(move || {
                let (read_ms, res) = read_and_decode(&book, i);
                let _ = tx.send((generation, i, read_ms, res));
                ctx.request_repaint();
            });
        }
        let keep = keep_range(self.page, count);
        self.cache.retain(|i, _| keep.contains(i));
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// 頁のバイトを読んで復号し、読み出し ms と結果を返す（復号 ms は stderr に出す）。
fn read_and_decode(book: &Book, i: usize) -> (f64, Result<Decoded, String>) {
    let t = Instant::now();
    let bytes = match book.read_page_bytes(i) {
        Ok(b) => b,
        Err(e) => return (ms(t), Err(e.to_string())),
    };
    let read_ms = ms(t);
    let t = Instant::now();
    let res = decode::decode_rgba(&bytes).map_err(|e| e.to_string());
    eprintln!(
        "page\t{}\tdecode {:.1} ms\t({} KB)",
        i + 1,
        ms(t),
        bytes.len() / 1024
    );
    (read_ms, res)
}

/// 復号結果をテクスチャにして頁キャッシュの 1 項目にする。`src` は統計行用（sync/prefetch）。
fn upload(
    ctx: &egui::Context,
    i: usize,
    read_ms: f64,
    res: Result<Decoded, String>,
    src: &str,
) -> Slot {
    let mut slot = Slot::default();
    match res {
        Ok(img) => {
            let t = Instant::now();
            let size = [img.width as usize, img.height as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, &img.rgba);
            let name = format!("page{i}");
            slot.texture = Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR));
            slot.note = img.repair.map(decode::Repair::note);
            eprintln!(
                "page\t{}\t{src}\tread {read_ms:.1} ms\ttexture {:.1} ms (CPU 側)",
                i + 1,
                ms(t)
            );
        }
        Err(msg) => slot.error = Some(msg),
    }
    slot
}

/// 1 頁分を `area` の中に収めて描く（エラー/修復の注記は上、画像は残りの中央）。
fn draw_slot(ui: &mut egui::Ui, area: egui::Rect, slot: &Slot) {
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        if let Some(msg) = &slot.error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("この頁は表示できません: {msg}"),
            );
        } else if let Some(tex) = &slot.texture {
            if let Some(note) = slot.note {
                ui.colored_label(egui::Color32::YELLOW, format!("修復して表示: {note}"));
            }
            let avail = ui.available_rect_before_wrap();
            let size = tex.size_vec2();
            let scale = (avail.width() / size.x)
                .min(avail.height() / size.y)
                .min(1.0);
            let rect = egui::Rect::from_center_size(avail.center(), size * scale);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
        }
    });
}

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

/// 日本語表示用に Windows の標準フォントを順に探して登録する（無ければ既定のまま）。
fn install_japanese_font(ctx: &egui::Context) {
    match find_japanese_font(Path::new(r"C:\Windows\Fonts")) {
        Some((name, data)) => {
            eprintln!("font\t{name}\t{} bytes", data.len());
            register_font(ctx, data);
        }
        None => eprintln!("font\t(none)\t既定フォントのまま（日本語は豆腐になりうる）"),
    }
}

/// `dir` から最初に読めた候補フォントの（ファイル名, 中身）。無ければ `None`。
fn find_japanese_font(dir: &Path) -> Option<(&'static str, Vec<u8>)> {
    ["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"]
        .into_iter()
        .find_map(|name| Some((name, std::fs::read(dir.join(name)).ok()?)))
}

fn register_font(ctx: &egui::Context, data: Vec<u8>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "jp".into(),
        std::sync::Arc::new(egui::FontData::from_owned(data)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("jp".into());
    }
    ctx.set_fonts(fonts);
}

/// 空の窓を開く（zip / フォルダのドロップ待ち）。窓が閉じるまで戻らない。
pub fn run_empty(exit_after: Option<Duration>) -> anyhow::Result<()> {
    let mut app = ViewApp::empty();
    app.deadline = exit_after.map(|d| Instant::now() + d);
    launch(app)
}

/// 窓を開いて閲覧する。窓が閉じるまで戻らない。`exit_after` があればその時間で自動で閉じる。
///
/// `page`（0 始まり）が頁数以上なら**最後の頁**に丸める（先頭には戻らない・エラーにもしない）。
/// 頁の無い冊は 0 頁目扱いで、窓は開く。`serve` の `?p=` は範囲外が 404 になる点だけ異なる。
pub fn run(
    title: String,
    book: Book,
    page: usize,
    exit_after: Option<Duration>,
) -> anyhow::Result<()> {
    launch(ViewApp::new(title, book, page, exit_after))
}

/// 組み立て済みの [`ViewApp`] を窓で動かす。窓が閉じるまで戻らない。
pub fn launch(app: ViewApp) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("manga-viewer")
            .with_inner_size([900.0, 1000.0]),
        ..Default::default()
    };
    eframe::run_native(
        "manga-viewer",
        options,
        Box::new(|cc| {
            install_japanese_font(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow!("ウィンドウを開けません: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `key` を 1 回押した（離した）フレームの `nav_delta` を、ヘッドレスの egui で測る。
    fn delta_for(key: egui::Key) -> isize {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut delta = 0;
        let mut out = ctx.run_ui(raw, |ui| delta = ui.ctx().input(nav_delta));
        out.textures_delta.clear(); // 描画しないので未適用の差分を捨てる
        delta
    }

    #[test]
    fn keys_map_to_page_direction() {
        assert_eq!(delta_for(egui::Key::ArrowRight), 1);
        assert_eq!(delta_for(egui::Key::PageDown), 1);
        assert_eq!(delta_for(egui::Key::Space), 1);
        assert_eq!(delta_for(egui::Key::ArrowLeft), -1);
        assert_eq!(delta_for(egui::Key::PageUp), -1);
        assert_eq!(delta_for(egui::Key::A), 0);
    }

    #[test]
    fn step_clamps_at_both_ends() {
        assert_eq!(step(0, -1, 79), 0);
        assert_eq!(step(0, 1, 79), 1);
        assert_eq!(step(78, 1, 79), 78);
        assert_eq!(step(200, 0, 79), 78);
        assert_eq!(step(0, 1, 0), 0);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mv-view-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn font_search_is_none_when_dir_has_no_candidates() {
        let dir = scratch("none");
        assert!(find_japanese_font(&dir).is_none());
        assert!(find_japanese_font(&dir.join("missing")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn font_search_prefers_earlier_candidate() {
        let dir = scratch("order");
        std::fs::write(dir.join("msgothic.ttc"), b"c").unwrap();
        assert_eq!(find_japanese_font(&dir).unwrap().0, "msgothic.ttc");
        std::fs::write(dir.join("meiryo.ttc"), b"b").unwrap();
        assert_eq!(find_japanese_font(&dir).unwrap().0, "meiryo.ttc");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
