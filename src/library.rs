//! 冊の一覧画面（設計 p0-3a・§3 の 1、#1）。
//!
//! 索引 DB の冊を表紙グリッドで出し、選んで開く。行は `show_rows` で仮想化し、見えている行の
//! 表紙だけを DB から読んでテクスチャにする（上限 [`MAX_TEXTURES`] 枚、古い物から捨てる）。
//! 表紙が無い冊があれば、裏スレッドで `Index::generate_covers` を回し、できた分から出す。
//! 壊れた冊は×印・赤字で出し、開こうとすると理由を状態欄に出す（設計 A-6）。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::index::{BookRow, Index, Status};

/// 1 マスの幅（表紙 + 余白）。
pub const CELL_W: f32 = 200.0;
/// 1 マスの高さ（表紙 + 題名 2 行 + 頁数）。
pub const CELL_H: f32 = 300.0;
/// マスの中の表紙の枠（設計の表紙上限 240×340 を縮めて出す）。
const THUMB: egui::Vec2 = egui::vec2(180.0, 240.0);
/// 表紙テクスチャを持つ上限（設計 p0-3a の LRU 300 枚）。
pub const MAX_TEXTURES: usize = 300;
/// 1 フレームに DB から読んで復号する表紙の数（残りは次フレーム）。
const COVERS_PER_FRAME: usize = 24;
/// 裏で表紙を作っている間、「まだ無い」表紙を DB に問い直す間隔。
const RECHECK: Duration = Duration::from_millis(500);

/// 窓幅に入る列数（最小 1）。
#[must_use]
pub fn grid_columns(width: f32) -> usize {
    ((width / CELL_W).floor() as usize).max(1)
}

/// 選択の移動。`dx` は左右（±1）、`dy` は上下の行数。端で止まる。
#[must_use]
pub fn move_selection(sel: usize, dx: isize, dy: isize, cols: usize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let delta = dx + dy * cols as isize;
    sel.saturating_add_signed(delta).min(count - 1)
}

/// 一覧で起きたこと。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// この冊を開く
    Open(BookRow),
    /// 窓を閉じる
    Close,
}

enum CoverTex {
    Ready(egui::TextureHandle),
    /// DB にまだ無い（作成中かもしれない）。`at` に確かめた
    Missing {
        at: Instant,
    },
    /// 作れなかった / 読めなかった
    Failed,
}

pub struct Library {
    db: PathBuf,
    root: Option<PathBuf>,
    index: Option<Index>,
    rows: Vec<BookRow>,
    selected: usize,
    covers: HashMap<i64, CoverTex>,
    /// テクスチャを作った順（古い物から捨てる）
    lru: VecDeque<i64>,
    status: Option<String>,
    /// 裏で表紙を作っている間 true
    thumbs_running: Arc<AtomicBool>,
    /// 前フレームのスクロール位置と見えている高さ（キーで選択を動かしたとき見える所へ送る）
    scroll: (f32, f32),
    scroll_to_selected: bool,
    /// 前フレームで見えていて、表紙をまだ出せていない（読み待ち・作成待ちの）マスの数
    covers_pending: usize,
}

impl Library {
    /// 索引 DB から一覧を読む。`root` があればその下だけ。
    pub fn open(db: &Path, root: Option<&Path>) -> anyhow::Result<Self> {
        let index = Index::open(db)?;
        let rows = match root {
            Some(root) => index.books_under(root)?,
            None => index.books()?,
        };
        Ok(Self {
            db: db.to_path_buf(),
            root: root.map(Path::to_path_buf),
            index: Some(index),
            rows,
            selected: 0,
            covers: HashMap::new(),
            lru: VecDeque::new(),
            status: None,
            thumbs_running: Arc::new(AtomicBool::new(false)),
            scroll: (0.0, 0.0),
            scroll_to_selected: false,
            covers_pending: 0,
        })
    }

    /// 表紙が無い冊の表紙を裏スレッドで作り始める（一覧の順 = 上から）。
    pub fn spawn_cover_generation(&self) {
        if self.thumbs_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let (db, root, running) = (
            self.db.clone(),
            self.root.clone(),
            self.thumbs_running.clone(),
        );
        std::thread::spawn(move || {
            let result =
                Index::open(&db).and_then(|mut index| index.generate_covers(root.as_deref(), None));
            match result {
                Ok(r) => eprintln!(
                    "covers\tgenerated={} failed={} elapsed_ms={}",
                    r.generated,
                    r.failed,
                    r.elapsed.as_millis()
                ),
                Err(e) => eprintln!("covers\t表紙を作れません: {e}"),
            }
            running.store(false, Ordering::SeqCst);
        });
    }

    /// 索引 DB のパスと root（開いた冊の次冊/前冊を索引順で探すのに使う）。
    #[must_use]
    pub fn source(&self) -> (PathBuf, Option<PathBuf>) {
        (self.db.clone(), self.root.clone())
    }

    #[must_use]
    pub fn rows(&self) -> &[BookRow] {
        &self.rows
    }

    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 前フレームで見えていたマスの表紙が出そろったか（作れなかった冊は数えない。#3 の測定用）。
    #[must_use]
    pub fn covers_settled(&self) -> bool {
        self.covers_pending == 0
    }

    /// 持っている表紙テクスチャの数。
    #[must_use]
    pub fn cover_textures(&self) -> usize {
        self.lru.len()
    }

    /// 表紙テクスチャ（もう読んであれば）。冊を開いた直後の仮表示に使う（#8）。
    #[must_use]
    pub fn cover_handle(&self, id: i64) -> Option<egui::TextureHandle> {
        match self.covers.get(&id) {
            Some(CoverTex::Ready(tex)) => Some(tex.clone()),
            _ => None,
        }
    }

    /// 状態欄の文言（壊れた冊の理由など）。
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(text.into());
    }

    /// `path` の冊を選んで見える所へ送る（閲覧から戻ったとき、読んでいた冊に合わせる）。
    pub fn select_path(&mut self, path: &Path) {
        if let Some(i) = self.rows.iter().position(|r| r.path == path) {
            self.selected = i;
            self.scroll_to_selected = true;
        }
    }

    /// 1 フレーム分の描画と入力。
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let ctx = ui.ctx().clone();
        // 文字を選べるラベルはドラッグを取るので、一覧の上のドラッグを窓の移動に使えるよう選べなくする
        ui.style_mut().interaction.selectable_labels = false;
        let cols = grid_columns(ui.available_width());
        let mut action = self.handle_keys(&ctx, cols);

        let running = self.thumbs_running.load(Ordering::SeqCst);
        let head = format!(
            "{}  {} 冊{}",
            self.root
                .as_deref()
                .map_or_else(|| "（全ルート）".to_string(), |r| r.display().to_string()),
            self.rows.len(),
            if running {
                " / 表紙を作成中…"
            } else {
                ""
            }
        );
        ui.label(head);
        if let Some(status) = &self.status {
            ui.colored_label(egui::Color32::LIGHT_RED, status);
        }
        if self.rows.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("索引に冊がありません（`manga-viewer index <root>` を先に実行）")
            });
            return action;
        }

        let n_rows = self.rows.len().div_ceil(cols);
        // 一覧の上のドラッグは窓の移動に使うので、ドラッグでのスクロールはしない（ホイール・スクロールバーは使える）
        let mut area = egui::ScrollArea::vertical()
            .id_salt("library-grid")
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..egui::scroll_area::ScrollSource::ALL
            });
        if std::mem::take(&mut self.scroll_to_selected) {
            let top = (self.selected / cols) as f32 * CELL_H;
            let (offset, height) = self.scroll;
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if top + CELL_H > offset + height && height > 0.0 {
                area = area.vertical_scroll_offset(top + CELL_H - height);
            }
        }
        let mut budget = COVERS_PER_FRAME;
        self.covers_pending = 0;
        let mut clicked = None;
        let out = area.show_rows(ui, CELL_H, n_rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    // 列数は CELL_W だけで割るので、マスの間に隙間を入れない（入れると右端の列が欠ける）
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for c in 0..cols {
                        let i = r * cols + c;
                        if i >= self.rows.len() {
                            break;
                        }
                        let (rect, resp) = ui
                            .allocate_exact_size(egui::vec2(CELL_W, CELL_H), egui::Sense::click());
                        if resp.clicked() {
                            clicked = Some((i, false));
                        }
                        if resp.double_clicked() {
                            clicked = Some((i, true));
                        }
                        if ui.is_rect_visible(rect) {
                            self.draw_cell(ui, &ctx, rect, i, &mut budget);
                        }
                    }
                });
            }
        });
        self.scroll = (out.state.clamped_offset().y, out.inner_rect.height());
        if budget == 0 || running {
            ctx.request_repaint_after(if budget == 0 { Duration::ZERO } else { RECHECK });
        }
        if let Some((i, double)) = clicked {
            self.selected = i;
            if double {
                action = action.or_else(|| self.open_selected());
            }
        }
        action
    }

    /// 矢印で選択、Enter で開く、Esc で閉じる。
    fn handle_keys(&mut self, ctx: &egui::Context, cols: usize) -> Option<Action> {
        let n = self.rows.len();
        let (dx, dy, home, end, enter, esc) = ctx.input(|i| {
            let k = |key| i.key_pressed(key);
            let page = 3;
            (
                isize::from(k(egui::Key::ArrowRight)) - isize::from(k(egui::Key::ArrowLeft)),
                isize::from(k(egui::Key::ArrowDown)) - isize::from(k(egui::Key::ArrowUp))
                    + page
                        * (isize::from(k(egui::Key::PageDown)) - isize::from(k(egui::Key::PageUp))),
                k(egui::Key::Home),
                k(egui::Key::End),
                k(egui::Key::Enter),
                k(egui::Key::Escape),
            )
        });
        if esc {
            return Some(Action::Close);
        }
        let before = self.selected;
        if dx != 0 || dy != 0 {
            self.selected = move_selection(self.selected, dx, dy, cols, n);
        }
        if home {
            self.selected = 0;
        }
        if end {
            self.selected = n.saturating_sub(1);
        }
        if self.selected != before {
            self.status = None;
            self.scroll_to_selected = true;
        }
        if enter {
            return self.open_selected();
        }
        None
    }

    /// 選んでいる冊を開く。壊れていれば理由を状態欄に出して開かない。
    fn open_selected(&mut self) -> Option<Action> {
        let row = self.rows.get(self.selected)?;
        if row.status == Status::Broken {
            self.status = Some(format!(
                "開けません（{}）: {}",
                row.title,
                row.error.as_deref().unwrap_or("壊れた冊です")
            ));
            return None;
        }
        self.status = None;
        Some(Action::Open(row.clone()))
    }

    fn draw_cell(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        rect: egui::Rect,
        i: usize,
        budget: &mut usize,
    ) {
        let row = self.rows[i].clone();
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        if i == self.selected {
            painter.rect_filled(rect.shrink(2.0), 6.0, visuals.selection.bg_fill);
        }
        let thumb_rect =
            egui::Rect::from_min_size(rect.min + egui::vec2((CELL_W - THUMB.x) / 2.0, 8.0), THUMB);
        let broken = row.status == Status::Broken;
        match (broken, self.cover_texture(ctx, row.id, budget)) {
            (false, Some(tex)) => {
                let size = tex.size_vec2();
                let scale = (THUMB.x / size.x).min(THUMB.y / size.y);
                let r = egui::Rect::from_center_size(thumb_rect.center(), size * scale);
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(tex.id(), r, uv, egui::Color32::WHITE);
            }
            _ => {
                if !broken && !matches!(self.covers.get(&row.id), Some(CoverTex::Failed)) {
                    self.covers_pending += 1;
                }
                painter.rect_filled(thumb_rect, 4.0, egui::Color32::from_gray(48));
                if broken {
                    painter.text(
                        thumb_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        egui::FontId::proportional(64.0),
                        egui::Color32::LIGHT_RED,
                    );
                }
            }
        }
        let color = if broken {
            egui::Color32::LIGHT_RED
        } else {
            visuals.text_color()
        };
        let text_top = thumb_rect.bottom() + 6.0;
        let galley = ui.painter().layout(
            row.title.clone(),
            egui::FontId::proportional(14.0),
            color,
            CELL_W - 16.0,
        );
        // 題名は 2 行まで（それ以上は切る）
        let title_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 8.0, text_top),
            egui::vec2(CELL_W - 16.0, 36.0),
        );
        ui.painter_at(title_rect)
            .galley(title_rect.min, galley, color);
        let pages = if broken {
            "壊れた冊".to_string()
        } else {
            format!("{} 頁", row.page_count)
        };
        painter.text(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 8.0),
            egui::Align2::LEFT_BOTTOM,
            pages,
            egui::FontId::proportional(12.0),
            if broken {
                egui::Color32::LIGHT_RED
            } else {
                visuals.weak_text_color()
            },
        );
    }

    /// 表紙テクスチャ（無ければ DB から読む。1 フレームの読み数は `budget` まで）。
    fn cover_texture(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        budget: &mut usize,
    ) -> Option<egui::TextureHandle> {
        match self.covers.get(&id) {
            Some(CoverTex::Ready(tex)) => return Some(tex.clone()),
            Some(CoverTex::Failed) => return None,
            Some(CoverTex::Missing { at }) if at.elapsed() < RECHECK => return None,
            _ => {}
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let entry = match self.index.as_ref().map(|ix| ix.cover(id)) {
            Some(Ok(Some(c))) => match image::load_from_memory(&c.jpeg) {
                Ok(img) => {
                    let size = [img.width() as usize, img.height() as usize];
                    let rgba = crate::decode::into_rgba(img);
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                    let tex =
                        ctx.load_texture(format!("cover{id}"), color, egui::TextureOptions::LINEAR);
                    self.remember(id);
                    CoverTex::Ready(tex)
                }
                Err(_) => CoverTex::Failed,
            },
            // 裏で作っている最中なら後で問い直す。作り終わっていれば作れなかった冊
            Some(Ok(None)) if self.thumbs_running.load(Ordering::SeqCst) => {
                CoverTex::Missing { at: Instant::now() }
            }
            _ => CoverTex::Failed,
        };
        let tex = match &entry {
            CoverTex::Ready(tex) => Some(tex.clone()),
            _ => None,
        };
        self.covers.insert(id, entry);
        tex
    }

    /// テクスチャを持った順を覚え、上限を超えたら古い物を捨てる（次に見えたとき読み直す）。
    fn remember(&mut self, id: i64) {
        self.lru.push_back(id);
        while self.lru.len() > MAX_TEXTURES {
            if let Some(old) = self.lru.pop_front() {
                self.covers.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_width() {
        assert_eq!(grid_columns(0.0), 1);
        assert_eq!(grid_columns(199.0), 1);
        assert_eq!(grid_columns(200.0), 1);
        assert_eq!(grid_columns(900.0), 4);
    }

    #[test]
    fn selection_moves_and_clamps() {
        // 4 列・10 冊
        assert_eq!(move_selection(0, 1, 0, 4, 10), 1);
        assert_eq!(move_selection(0, -1, 0, 4, 10), 0);
        assert_eq!(move_selection(1, 0, 1, 4, 10), 5);
        assert_eq!(move_selection(5, 0, 1, 4, 10), 9); // 最終行は端で止まる
        assert_eq!(move_selection(5, 0, -1, 4, 10), 1);
        assert_eq!(move_selection(1, 0, -1, 4, 10), 0);
        assert_eq!(move_selection(0, 1, 0, 4, 0), 0);
    }
}
