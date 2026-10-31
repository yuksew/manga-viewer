//! 単独ウィンドウの閲覧（p0-6b）。`manga-viewer view` から呼ぶ。
//!
//! 1 冊を開いて 1 頁を窓に収めて表示し、→/PageDown/Space で次、←/PageUp で前、Esc で終了。
//! 復号は [`crate::decode`]（`imgcheck` を通ったバイトだけ）。壊れた頁は画像を出さずエラー文言を出す。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use eframe::egui;

use crate::book::Book;
use crate::decode::{self, Decoded};
use crate::index::{Index, Status};
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

/// このフレームの入力から頁送りの向きを決める（次 +1 / 前 -1、両方や無入力は 0）。
fn nav_delta(i: &egui::InputState) -> isize {
    let next = i.key_pressed(egui::Key::ArrowRight)
        || i.key_pressed(egui::Key::PageDown)
        || i.key_pressed(egui::Key::Space);
    let prev = i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp);
    isize::from(next) - isize::from(prev)
}

pub struct ViewApp {
    title: String,
    /// 開いている冊。空の窓（ドロップ待ち）では `None`
    book: Option<Book>,
    page: usize,
    /// ドロップで開けなかったときの文言
    open_error: Option<String>,
    /// 現在頁のテクスチャ。壊れ頁では `None`
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    /// 現在頁が修復して表示されているときの注記（頁を送ると消える）
    note: Option<&'static str>,
    loaded: Option<usize>,
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
            open_error: None,
            texture: None,
            error: None,
            note: None,
            loaded: None,
            deadline: exit_after.map(|d| Instant::now() + d),
        }
    }

    /// 冊のない空の窓（ドロップ待ち）。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            title: String::new(),
            book: None,
            page: 0,
            open_error: None,
            texture: None,
            error: None,
            note: None,
            loaded: None,
            deadline: None,
        }
    }

    /// `path`（zip かフォルダ）を開いて今の冊と入れ替える。失敗なら今の表示は保ち、文言を出す。
    pub fn open_dropped(&mut self, path: &Path) {
        match open_path(path) {
            Ok((title, book)) => {
                self.title = title;
                self.book = Some(book);
                self.page = 0;
                self.open_error = None;
                self.texture = None;
                self.error = None;
                self.note = None;
                self.loaded = None;
            }
            Err(err) => self.open_error = Some(format!("{err:#}")),
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

        let count = self.book.as_ref().map_or(0, Book::page_count);
        let delta = ctx.input(nav_delta);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.page = step(self.page, delta, count);
        self.ensure_loaded(ctx);

        ui.label(format!("{}  {}/{}", self.title, self.page + 1, count));
        if let Some(msg) = &self.error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("この頁は表示できません: {msg}"),
            );
        } else if let Some(tex) = &self.texture {
            if let Some(note) = self.note {
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

    fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if self.loaded == Some(self.page) {
            return;
        }
        self.loaded = Some(self.page);
        let Some(book) = &self.book else { return };
        match load_page(book, self.page) {
            Ok(img) => {
                let size = [img.width as usize, img.height as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, &img.rgba);
                self.texture = Some(ctx.load_texture("page", color, egui::TextureOptions::LINEAR));
                self.note = img.repair.map(decode::Repair::note);
                self.error = None;
            }
            Err(msg) => {
                self.texture = None;
                self.note = None;
                self.error = Some(msg);
            }
        }
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

fn launch(app: ViewApp) -> anyhow::Result<()> {
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
