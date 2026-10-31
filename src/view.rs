//! 単独ウィンドウの閲覧（p0-6b）。`manga-viewer view` から呼ぶ。
//!
//! 1 冊を開いて 1 頁を窓に収めて表示し、→/PageDown/Space で次、←/PageUp で前、Esc で終了。
//! 復号は [`crate::decode`]（`imgcheck` を通ったバイトだけ）。壊れた頁は画像を出さずエラー文言を出す。

use std::path::{Path, PathBuf};
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

/// 左頁 `page` から `delta` 回送る。見開きは 1 回 2 頁、最終頁判定は右側の頁で行う。
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

/// 表示中の 1 頁分（見開きでは左右 2 つ）。
#[derive(Default)]
struct Slot {
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    /// 修復して表示されているときの注記
    note: Option<&'static str>,
}

pub struct ViewApp {
    title: String,
    /// 開いている冊。空の窓（ドロップ待ち）では `None`
    book: Option<Book>,
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
    slots: Vec<Slot>,
    /// 窓ドラッグ移動の状態
    drag: Drag,
    /// 読み込み済みの（左頁, 見開きか）
    loaded: Option<(usize, bool)>,
    deadline: Option<Instant>,
}

impl ViewApp {
    /// `page` は範囲外なら最後の頁に丸める（[`run`] と同じ）。
    #[must_use]
    pub fn new(title: String, book: Book, page: usize, exit_after: Option<Duration>) -> Self {
        let page = step(page, 0, book.page_count());
        Self {
            title,
            book: Some(book),
            page,
            deadline: exit_after.map(|d| Instant::now() + d),
            ..Self::empty()
        }
    }

    /// 冊のない空の窓（ドロップ待ち）。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            title: String::new(),
            book: None,
            page: 0,
            spread: false,
            prompt: None,
            notice: None,
            index: None,
            open_error: None,
            slots: Vec::new(),
            drag: Drag::default(),
            loaded: None,
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
                self.book = Some(book);
                self.page = 0;
                self.prompt = None;
                self.notice = None;
                self.open_error = None;
                self.slots.clear();
                self.loaded = None;
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
        let Some(count) = self.book.as_ref().map(Book::page_count) else {
            return;
        };
        let spread = spread_active(self.spread, ctx.content_rect().width());
        self.ensure_loaded(ctx, spread);

        let label = if spread && self.page + 1 < count {
            format!("{}-{}/{}", self.page + 1, self.page + 2, count)
        } else {
            format!("{}/{}", self.page + 1, count)
        };
        ui.label(format!("{}  {label}", self.title));
        if let Some(notice) = self.notice {
            ui.colored_label(egui::Color32::YELLOW, notice);
        }
        if self.slots.len() == 2 {
            ui.columns(2, |cols| {
                for (col, slot) in cols.iter_mut().zip(&self.slots) {
                    draw_slot(col, slot);
                }
            });
        } else if let Some(slot) = self.slots.first() {
            draw_slot(ui, slot);
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
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
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
        let count = self.book.as_ref().map_or(0, Book::page_count);
        let spread = spread_active(self.spread, ctx.content_rect().width());
        self.notice = None;
        match plan_nav(self.page, delta, spread, count) {
            Nav::Page(p) => self.page = p,
            Nav::Prompt(dir) => self.prompt = Some(dir),
        }
    }

    fn ensure_loaded(&mut self, ctx: &egui::Context, spread: bool) {
        if self.loaded == Some((self.page, spread)) {
            return;
        }
        self.loaded = Some((self.page, spread));
        self.slots.clear();
        let Some(book) = &self.book else { return };
        let last = book.page_count().saturating_sub(1);
        let upto = if spread {
            (self.page + 1).min(last)
        } else {
            self.page
        };
        for i in self.page..=upto {
            let mut slot = Slot::default();
            match load_page(book, i) {
                Ok(img) => {
                    let size = [img.width as usize, img.height as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, &img.rgba);
                    let name = format!("page{}", i - self.page);
                    slot.texture =
                        Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR));
                    slot.note = img.repair.map(decode::Repair::note);
                }
                Err(msg) => slot.error = Some(msg),
            }
            self.slots.push(slot);
        }
    }
}

/// 1 頁分を `ui` の空き領域に収めて描く。
fn draw_slot(ui: &mut egui::Ui, slot: &Slot) {
    if let Some(msg) = &slot.error {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!("この頁は表示できません: {msg}"),
        );
    } else if let Some(tex) = &slot.texture {
        if let Some(note) = slot.note {
            ui.colored_label(egui::Color32::YELLOW, format!("修復して表示: {note}"));
        }
        let avail = ui.available_size();
        let size = tex.size_vec2();
        let scale = (avail.x / size.x).min(avail.y / size.y).min(1.0);
        ui.centered_and_justified(|ui| {
            ui.add(egui::Image::new((tex.id(), size * scale)));
        });
    }
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
