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

struct ViewApp {
    title: String,
    book: Book,
    page: usize,
    /// 現在頁のテクスチャ。壊れ頁では `None`
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    loaded: Option<usize>,
    deadline: Option<Instant>,
}

impl ViewApp {
    fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if self.loaded == Some(self.page) {
            return;
        }
        self.loaded = Some(self.page);
        match load_page(&self.book, self.page) {
            Ok(img) => {
                let size = [img.width as usize, img.height as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, &img.rgba);
                self.texture = Some(ctx.load_texture("page", color, egui::TextureOptions::LINEAR));
                self.error = None;
            }
            Err(msg) => {
                self.texture = None;
                self.error = Some(msg);
            }
        }
    }
}

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let count = self.book.page_count();
        let delta = ctx.input(|i| {
            let next = i.key_pressed(egui::Key::ArrowRight)
                || i.key_pressed(egui::Key::PageDown)
                || i.key_pressed(egui::Key::Space);
            let prev = i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp);
            isize::from(next) - isize::from(prev)
        });
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
            let avail = ui.available_size();
            let size = tex.size_vec2();
            let scale = (avail.x / size.x).min(avail.y / size.y).min(1.0);
            ui.centered_and_justified(|ui| {
                ui.add(egui::Image::new((tex.id(), size * scale)));
            });
        }
    }
}

/// 日本語表示用に Windows の標準フォントを順に探して登録する（無ければ既定のまま）。
fn install_japanese_font(ctx: &egui::Context) {
    for name in ["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"] {
        let path = Path::new(r"C:\Windows\Fonts").join(name);
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "jp".into(),
            std::sync::Arc::new(egui::FontData::from_owned(data)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("jp".into());
        }
        ctx.set_fonts(fonts);
        return;
    }
}

/// 窓を開いて閲覧する。窓が閉じるまで戻らない。`exit_after` があればその時間で自動で閉じる。
pub fn run(
    title: String,
    book: Book,
    page: usize,
    exit_after: Option<Duration>,
) -> anyhow::Result<()> {
    let page = step(page, 0, book.page_count());
    let app = ViewApp {
        title,
        book,
        page,
        texture: None,
        error: None,
        loaded: None,
        deadline: exit_after.map(|d| Instant::now() + d),
    };
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
