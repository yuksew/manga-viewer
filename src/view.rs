//! 単独ウィンドウの閲覧（p0-6b）。`manga-viewer view` から呼ぶ。
//!
//! 1 冊を開いて 1 頁を窓に収めて表示し、→/PageDown/Space で次、←/PageUp で前、Esc で終了。
//! 一覧（[`crate::library`]）から開いたときは、Esc で一覧へ戻る。
//! 復号は [`crate::decode`]（`imgcheck` を通ったバイトだけ）。壊れた頁は画像を出さずエラー文言を出す。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use eframe::egui;

use crate::autopilot::{Autopilot, Mode, Observed, Probe};
use crate::book::Book;
use crate::decode::{self, Decoded};
use crate::drag::{Drag, DragInput};
use crate::index::{BookRow, Index, Status};
use crate::library::{Action, Library};
use crate::neighbor::{NavDir, resolve_neighbor};
use crate::scan::BookKind;

/// 窓のタイトルバーに出すアプリ名（冊を開くと「冊名 - アプリ名」）。
const APP_NAME: &str = "manga-viewer";

/// 窓の状態（大きさ・位置・見開き）の保存ファイル名（索引 DB と同じフォルダ、#13）。
const STATE_FILE: &str = "window.ron";
/// 保存ファイルの中の見開きの希望の鍵。
const SPREAD_KEY: &str = "spread";
/// 保存ファイルの中の、見開きの組をずらした冊（パスを 1 行ずつ、#16）の鍵。
const SHIFTED_KEY: &str = "shifted_books";

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

/// 窓の下端からこの距離（点）以内にカーソルがあると頁スライダーを出す。
pub const SLIDER_ZONE: f32 = 72.0;
/// 窓の上端からこの距離（点）以内にカーソルがあると頁番号・モードの表示を出す（#15）。
pub const TOP_ZONE: f32 = 48.0;
/// 頁スライダーの帯の高さ（点）。
const SLIDER_HEIGHT: f32 = 48.0;

/// 見開きにする最小の窓幅（これより狭いと自動で単頁）。
pub const SPREAD_MIN_WIDTH: f32 = 700.0;

/// 見開きの希望と窓幅から、実際に見開きにするか。
#[must_use]
pub fn spread_active(want: bool, width: f32) -> bool {
    want && width >= SPREAD_MIN_WIDTH
}

/// 頁の並べ方。`bool` からは単頁（`false`）/ 表紙を 1 枚で出す見開き（`true`）になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Single,
    /// 1 枚目を表紙として 1 枚で出し、以降を 2-3, 4-5… で組む（#11、多くの冊は 1 枚目が表紙）
    Spread,
    /// 組を 1 頁ずらし、1-2, 3-4… で組む（表紙が 1 枚目でない冊、#16）
    Shifted,
}

impl From<bool> for Layout {
    fn from(spread: bool) -> Self {
        if spread {
            Layout::Spread
        } else {
            Layout::Single
        }
    }
}

impl Layout {
    /// 見開きの希望・窓幅・組ずらしから実際の並べ方を決める。
    #[must_use]
    pub fn new(want: bool, width: f32, shifted: bool) -> Self {
        match (spread_active(want, width), shifted) {
            (false, _) => Layout::Single,
            (true, false) => Layout::Spread,
            (true, true) => Layout::Shifted,
        }
    }

    fn is_spread(self) -> bool {
        self != Layout::Single
    }
}

/// `page` を含む表示の組（先頭頁, 末尾頁）。単頁なら `(page, page)`。
/// 見開きは 1 枚目（索引 0）を表紙として 1 枚で出し、以降を索引 1-2, 3-4… で組む（多くの冊は 1 枚目が表紙）。
/// 組ずらし（[`Layout::Shifted`]）は索引 0-1, 2-3… で組む。最終頁が組の相手を持たなければ 1 枚。
#[must_use]
pub fn spread_pages(page: usize, layout: impl Into<Layout>, count: usize) -> (usize, usize) {
    let last = count.saturating_sub(1);
    let page = page.min(last);
    let start = match layout.into() {
        Layout::Single => return (page, page),
        Layout::Spread if page == 0 => return (0, 0),
        // 奇数索引が組の先頭（偶数索引は 1 つ前と組む）
        Layout::Spread => page - (page + 1) % 2,
        // 偶数索引が組の先頭
        Layout::Shifted => page - page % 2,
    };
    (start, (start + 1).min(last))
}

/// 見開きの状態を窓内に出す文言（切替が効いたか・効かない理由を利用者に見せる）。
#[must_use]
pub fn mode_label(
    want: bool,
    width: f32,
    active: impl Into<Layout>,
    page: usize,
    count: usize,
) -> String {
    let layout = active.into();
    let (start, end) = spread_pages(page, layout, count);
    let shifted = if layout == Layout::Shifted {
        "・組ずらし"
    } else {
        ""
    };
    if !want {
        "単頁（F で見開き）".to_string()
    } else if !layout.is_spread() {
        format!("単頁（窓幅 {width:.0} が {SPREAD_MIN_WIDTH:.0} 未満のため見開きにできません）")
    } else if start == end && start == 0 && count > 1 {
        "見開き（表紙は 1 枚・S で組をずらす）".to_string()
    } else if start == end {
        format!("見開き{shifted}（最終頁は 1 枚）")
    } else if layout == Layout::Shifted {
        "見開き・組ずらし（S で戻す）".to_string()
    } else {
        "見開き（F で単頁）".to_string()
    }
}

/// 先読みする頁。今の表示の次と前を 1 送り分ずつ、次を先に（範囲内のみ）。
#[must_use]
pub fn prefetch_plan(page: usize, layout: impl Into<Layout>, count: usize) -> Vec<usize> {
    let layout = layout.into();
    let span = if layout.is_spread() { 2 } else { 1 };
    let (start, end) = spread_pages(page, layout, count);
    let next = (end + 1..end + 1 + span * 2).filter(|&i| i < count);
    let prev = (start.saturating_sub(span * 2)..start).rev();
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

/// S（見開きの組ずらしトグル、#16）が押されたか。F と同じくテキストだけ来る場合も拾う。
fn shift_toggle_pressed(i: &egui::InputState) -> bool {
    i.key_pressed(egui::Key::S)
        || i.events.iter().any(
            |e| matches!(e, egui::Event::Text(t) if matches!(t.as_str(), "s" | "S" | "ｓ" | "Ｓ")),
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

/// 確認中に、確認の向きと逆へホイールしたか（次冊の確認中に上、前冊の確認中に下）。
/// 逆向きは「戻りたい」意思なので、確認を閉じる（いいえ扱い）。同じ向きのホイールは従来どおり無視。
fn wheel_reverses(i: &egui::InputState, dir: NavDir) -> bool {
    i.events.iter().any(|e| match e {
        egui::Event::MouseWheel { delta, .. } => match dir {
            NavDir::Next => delta.y > 0.0,
            NavDir::Prev => delta.y < 0.0,
        },
        _ => false,
    })
}

/// 次冊/前冊の確認パネルの寸法。窓が狭いときは余白・ボタン・見出しを縮めて窓に収める。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PromptLayout {
    /// パネルの幅（余白込み）
    pub panel: f32,
    /// パネル内側の余白
    pub margin: f32,
    /// ボタン 1 つの寸法
    pub button: egui::Vec2,
    /// ボタンの間
    pub gap: f32,
    /// 見出しの文字の大きさ
    pub title_size: f32,
}

impl PromptLayout {
    /// 広い窓でのパネル幅
    pub const WIDE: f32 = 476.0;
    /// パネルと窓の端の最小の隙間（片側）
    pub const EDGE: f32 = 12.0;
    /// 実寸を測る前の高さの見込み
    const HEIGHT_GUESS: f32 = 220.0;

    /// 窓幅 `width` に収まる寸法。
    #[must_use]
    pub fn fit(width: f32) -> Self {
        let panel = Self::WIDE.min(width - Self::EDGE * 2.0).max(200.0);
        let narrow = panel < Self::WIDE;
        let margin = if narrow { 16.0 } else { 28.0 };
        let gap = if narrow { 12.0 } else { 20.0 };
        let inner = panel - margin * 2.0;
        let button_w = ((inner - gap) / 2.0).min(170.0);
        Self {
            panel,
            margin,
            button: egui::vec2(button_w, 60.0),
            gap,
            // 見出し 10 文字が 1 行に収まる大きさ（全角 1 文字 ≒ 文字の大きさ）
            title_size: (inner / 10.5).clamp(18.0, 28.0),
        }
    }
}

/// 頁送りの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    /// 頁が変わる（または変わらない）
    Page(usize),
    /// 端を越えようとした。次冊/前冊の確認を出す
    Prompt(NavDir),
}

/// 先頭側の頁 `page` から `delta` 回送る。見開きは 1 回 1 組（[`spread_pages`] の組、表紙だけは 1 枚）。
/// 最終頁判定は今の組の末尾（右綴じなので左側の頁）、最初頁判定は組の先頭が 0 か。端の意味は単頁時と変わらない。
/// 端を越えるのは今いる組が端のときだけで、複数回の送りが途中で端に着いたらそこで止まる。
#[must_use]
pub fn plan_nav(page: usize, delta: isize, layout: impl Into<Layout>, count: usize) -> Nav {
    let layout = layout.into();
    let last = count.saturating_sub(1);
    let (mut start, mut end) = spread_pages(page, layout, count);
    if delta > 0 && end >= last {
        return Nav::Prompt(NavDir::Next);
    }
    if delta < 0 && start == 0 {
        return Nav::Prompt(NavDir::Prev);
    }
    for _ in 0..delta.unsigned_abs() {
        (start, end) = if delta > 0 && end < last {
            spread_pages(end + 1, layout, count)
        } else if delta < 0 && start > 0 {
            spread_pages(start - 1, layout, count)
        } else {
            break;
        };
    }
    Nav::Page(start)
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
    /// 元画像の寸法（点）。テクスチャは窓に合わせて縮めてあるので、描く大きさはこちらで決める
    size: egui::Vec2,
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
    /// 冊の一覧。あれば冊を開いていない間は一覧を出し、閲覧中の Esc で一覧へ戻る
    library: Option<Library>,
    /// 頁を縮める先の大きさ（窓の物理ピクセル）。今の窓から毎フレーム決める
    target: (u32, u32),
    /// キャッシュの頁を縮めたときの大きさ。窓がこれより広がったら作り直す
    decoded_for: (u32, u32),
    /// 冊を開いた直後、先頭頁ができるまで出す表紙（一覧の表紙テクスチャ）
    placeholder: Option<egui::TextureHandle>,
    /// 冊を開いた回数（測定で開き直しを見分ける）
    opens: u64,
    /// このフレームで頁の画面を描いたか（一覧から開いたフレームは描かない）
    drew_pages: bool,
    /// 通し測定（`MV_TRACE=1` か `--autopilot`、#3）
    probe: Option<Probe>,
    autopilot: Option<Autopilot>,
    /// 最後に OS のタイトルバーへ送った文字列（変わったときだけ送る）
    window_title: Option<String>,
    /// 頁スライダーをつまんでいる間の行き先（0 始まり）。離したときにその頁へ飛ぶ
    slider_drag: Option<usize>,
    /// 窓の状態（大きさ・位置・見開き）を次の起動に持ち越すか（#13）。[`ViewApp::persists`] で決まる
    persist: bool,
    /// このフレームに頁スライダーを出した範囲（その上のダブルクリックは全画面にしない）
    slider_rect: Option<egui::Rect>,
    /// 上部に出す表示の文字列（[`ViewApp::status`]）
    status: String,
    /// 今の冊の見開きの組をずらしているか（#16）
    shifted: bool,
    /// 組をずらした冊のパス。冊を開くたびにここから [`ViewApp::shifted`] を決め、終了時に保存する
    shifted_books: HashSet<PathBuf>,
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

    /// 窓の状態を次の起動に持ち越すか（#13）。測定（`MV_TRACE`・`--autopilot`）と自動で閉じる窓
    /// （`--exit-after-ms`）は毎回同じ条件で動かしたいので持ち越さない。`MV_PERSIST=0` でも切れる。
    #[must_use]
    pub fn persists(&self) -> bool {
        self.probe.is_none()
            && self.deadline.is_none()
            && std::env::var_os("MV_PERSIST").is_none_or(|v| v != "0")
    }

    /// 窓の状態を保存するかを決める（[`launch`] は [`ViewApp::persists`] で決める。試験用）。
    #[must_use]
    pub fn with_persist(mut self, on: bool) -> Self {
        self.persist = on;
        self
    }

    /// 前回保存した状態（見開きの希望）を戻す。窓の大きさ・位置は eframe が戻す。
    pub fn restore_state(&mut self, storage: &dyn eframe::Storage) {
        if let Some(v) = storage.get_string(SPREAD_KEY) {
            self.spread = v == "1";
        }
        if let Some(v) = storage.get_string(SHIFTED_KEY) {
            self.shifted_books = v
                .lines()
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .collect();
        }
        self.shifted = self.book_is_shifted();
    }

    /// 今の冊の見開きの組をずらしているか（#16）。
    #[must_use]
    pub fn shifted(&self) -> bool {
        self.shifted
    }

    fn book_is_shifted(&self) -> bool {
        self.book
            .as_deref()
            .is_some_and(|b| self.shifted_books.contains(b.path()))
    }

    /// 見開きの組ずらしを切り替え、冊ごとに覚える。
    fn toggle_shift(&mut self) {
        let Some(path) = self.book.as_deref().map(|b| b.path().to_path_buf()) else {
            return;
        };
        self.shifted = !self.shifted;
        if self.shifted {
            self.shifted_books.insert(path);
        } else {
            self.shifted_books.remove(&path);
        }
    }

    /// 窓幅 `width` での並べ方。
    fn layout(&self, width: f32) -> Layout {
        Layout::new(self.spread, width, self.shifted)
    }

    /// 見開きの希望（窓が狭いと実際は単頁）。
    #[must_use]
    pub fn spread_wanted(&self) -> bool {
        self.spread
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
            library: None,
            target: (0, 0),
            decoded_for: (0, 0),
            placeholder: None,
            opens: 0,
            drew_pages: false,
            probe: None,
            autopilot: None,
            window_title: None,
            slider_drag: None,
            persist: false,
            slider_rect: None,
            status: String::new(),
            shifted: false,
            shifted_books: HashSet::new(),
        }
    }

    /// 冊の一覧から始める窓（#1）。次冊/前冊は一覧と同じ索引の順で探す。
    #[must_use]
    pub fn with_library(library: Library) -> Self {
        let (db, root) = library.source();
        let mut app = Self::empty().with_index(db, root);
        app.library = Some(library);
        app
    }

    /// 一覧を出しているか（冊を開いていない・一覧がある）。
    #[must_use]
    pub fn in_library(&self) -> bool {
        self.book.is_none() && self.library.is_some()
    }

    /// 一覧（あれば）。
    #[must_use]
    pub fn library(&self) -> Option<&Library> {
        self.library.as_ref()
    }

    /// 開いている冊を閉じて一覧へ戻る。一覧では読んでいた冊を選んでおく。
    fn back_to_library(&mut self) {
        let path = self.book.take().map(|b| b.path().to_path_buf());
        self.placeholder = None;
        self.prompt = None;
        self.notice = None;
        self.open_error = None;
        self.cache.clear();
        self.inflight.clear();
        self.visible.clear();
        self.generation += 1;
        if let (Some(lib), Some(path)) = (self.library.as_mut(), path) {
            lib.select_path(&path);
        }
    }

    /// 一覧で選ばれた冊を開く。開けなければ一覧の状態欄に理由を出す。
    fn open_row(&mut self, row: &BookRow) {
        match Book::open(&row.path, row.kind) {
            Ok(book) => {
                self.set_book(row.title.clone(), book);
                // 先頭頁の復号を待つ間、一覧の表紙を出す（#8）
                self.placeholder = self
                    .library
                    .as_ref()
                    .and_then(|lib| lib.cover_handle(row.id));
            }
            Err(err) => {
                if let Some(lib) = self.library.as_mut() {
                    lib.set_status(format!("開けません（{}）: {err}", row.title));
                }
            }
        }
    }

    /// 今の冊と入れ替えて先頭頁から読む。
    fn set_book(&mut self, title: String, book: Book) {
        self.placeholder = None;
        self.title = title;
        self.book = Some(Arc::new(book));
        self.shifted = self.book_is_shifted();
        self.page = 0;
        self.prompt = None;
        self.notice = None;
        self.open_error = None;
        self.cache.clear();
        self.inflight.clear();
        self.visible.clear();
        self.generation += 1;
        self.opens += 1;
    }

    /// 測定を付ける（#3）。`autopilot` があればキーを注入して場面を再現し、終われば窓を閉じる。
    #[must_use]
    pub fn with_probe(mut self, probe: Probe, autopilot: Option<Autopilot>) -> Self {
        self.probe = Some(probe);
        self.autopilot = autopilot;
        self
    }

    /// 測定が見るこのフレームの状態。
    fn observed(&self) -> Observed {
        let drew = self.drew_pages;
        Observed {
            in_library: self.in_library(),
            covers_settled: self.library.as_ref().is_some_and(Library::covers_settled),
            opens: self.opens,
            page: self.page,
            page_ready: drew
                && !self.visible.is_empty()
                && self.visible.iter().all(|i| self.cache.contains_key(i)),
            placeholder: drew && self.placeholder.is_some(),
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

    /// 冊を開いた直後の表紙を仮表示しているか（先頭頁の復号待ち）。
    #[must_use]
    pub fn showing_placeholder(&self) -> bool {
        self.placeholder.is_some() && !self.cache.contains_key(&0)
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
            Ok((title, book)) => self.set_book(title, book),
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

    /// 確認を窓中央に浮かせて出す（背景は暗くする）。Enter=はい / Esc=いいえ / 逆向きホイール=閉じる。
    /// `keys` が偽（確認を出したフレーム）は入力で答えない。
    fn show_prompt(&mut self, ui: &mut egui::Ui, dir: NavDir, keys: bool) {
        let ctx = ui.ctx().clone();
        let mut answer = None;
        if !keys {
        } else if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            answer = Some(true);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape) || wheel_reverses(i, dir)) {
            answer = Some(false);
        }

        let dim = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("nav-prompt-dim"));
        ctx.layer_painter(dim).rect_filled(
            ctx.content_rect(),
            0.0,
            egui::Color32::from_black_alpha(150),
        );

        let (head, title, back) = match dir {
            NavDir::Next => ("最後の頁です", "次の冊を開きますか？", "上ホイールで閉じる"),
            NavDir::Prev => ("最初の頁です", "前の冊を開きますか？", "下ホイールで閉じる"),
        };
        let screen = ctx.content_rect();
        let lay = PromptLayout::fit(screen.width());
        // Area は初回フレームを寸法測りで不可視にするので使わず、前面レイヤーの子 Ui を窓中央に置く
        // （出したフレームから見える）。高さは中身次第なので前フレームの実寸で中央に寄せる
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("nav-prompt"));
        let height_id = egui::Id::unique("nav-prompt-height");
        let height = ctx
            .data(|d| d.get_temp::<f32>(height_id))
            .unwrap_or(PromptLayout::HEIGHT_GUESS);
        let rect = egui::Rect::from_center_size(screen.center(), egui::vec2(lay.panel, height));
        let mut panel_ui = ui.new_child(egui::UiBuilder::new().layer_id(layer).max_rect(rect));
        let inner = lay.panel - lay.margin * 2.0;
        let shown = egui::Frame::popup(panel_ui.style())
            .inner_margin(egui::Margin::same(lay.margin as i8))
            .corner_radius(14)
            .show(&mut panel_ui, |ui| {
                ui.set_width(inner);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(head).size(16.0).weak());
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(title).size(lay.title_size).strong());
                    ui.add_space(20.0);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = lay.gap;
                    ui.add_space((inner - lay.button.x * 2.0 - lay.gap) / 2.0);
                    let yes = egui::Button::new(egui::RichText::new("はい").size(24.0).strong())
                        .min_size(lay.button)
                        .fill(ui.visuals().selection.bg_fill);
                    if ui.add(yes).clicked() {
                        answer = Some(true);
                    }
                    let no = egui::Button::new(egui::RichText::new("いいえ").size(24.0))
                        .min_size(lay.button);
                    if ui.add(no).clicked() {
                        answer = Some(false);
                    }
                });
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(if lay.panel < PromptLayout::WIDE {
                            // 狭いときは句の途中で折り返さないよう、ホイールの案内を次の行に出す
                            format!(
                                "Enter = はい ／ Esc = いいえ
{back}"
                            )
                        } else {
                            format!("Enter = はい ／ Esc = いいえ ／ {back}")
                        })
                        .size(14.0)
                        .weak(),
                    );
                });
            });
        let measured = shown.response.rect.height();
        if (measured - height).abs() > 0.5 {
            ctx.data_mut(|d| d.insert_temp(height_id, measured));
            ctx.request_repaint();
        }
        if let Some(yes) = answer {
            self.answer_prompt(yes);
        }
    }

    /// 1 フレーム分の描画（入力処理・頁読み込み・ラベル/画像/エラー文言）。
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.show_frame(ui);
        self.sync_window_title(ui.ctx());
    }

    /// タイトルバーの文字列。冊を開いていれば冊名を出す。
    #[must_use]
    pub fn window_title(&self) -> String {
        if self.book.is_some() && !self.title.is_empty() {
            format!("{} - {APP_NAME}", self.title)
        } else {
            APP_NAME.to_string()
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let want = self.window_title();
        if self.window_title.as_deref() != Some(want.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(want.clone()));
            self.window_title = Some(want);
        }
    }

    fn show_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.drew_pages = false;
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
        self.handle_fullscreen(ctx);
        self.update_window_drag(ctx);
        self.slider_rect = None;
        if let Some(err) = &self.open_error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
        if self.book.is_none()
            && let Some(lib) = self.library.as_mut()
        {
            match lib.show(ui) {
                Some(Action::Open(row)) => self.open_row(&row),
                Some(Action::Close) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                None => {}
            }
            // 開いた同じフレームの Enter/Esc を閲覧側で拾わない
            return;
        }
        if self.book.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.centered_and_justified(|ui| ui.label(DROP_GUIDE));
            return;
        }

        // 確認中は他の送り入力（キー/ホイール）を見ない。出した同じフレームの Enter/Esc/ホイールも見ない
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
        let layout = self.layout(width);
        self.ensure_loaded(ctx, layout);
        self.drew_pages = true;

        let label = match (self.visible.first(), self.visible.last()) {
            (Some(a), Some(b)) if a != b => format!("{}-{}/{}", a + 1, b + 1, count),
            _ => format!("{}/{}", self.page + 1, count),
        };
        let mode = mode_label(self.spread, width, layout, self.page, count);
        // 全画面ではタイトルバーが無いので冊名もここに出す
        self.status = if is_fullscreen(ctx) {
            format!("{}  {label}  [{mode}]", self.title)
        } else {
            format!("{label}  [{mode}]")
        };
        // 左右の領域を明示的に切り、各頁をその中へ描く（レイアウト任せにしない）。
        // 頁番号などの表示は頁の上に重ねるので、頁は窓いっぱいに使う
        let area = ui.available_rect_before_wrap();
        let w = area.width() / self.visible.len().max(1) as f32;
        let n = self.visible.len();
        for (k, i) in self.visible.iter().enumerate() {
            let Some(slot) = self.cache.get(i) else {
                // 先頭頁の復号待ちは一覧の表紙を引き伸ばして出す（#8）
                if let (0, Some(tex)) = (*i, &self.placeholder) {
                    let k = if n == 2 { 1 - k } else { k };
                    let min = area.min + egui::vec2(w * k as f32, 0.0);
                    let rect = egui::Rect::from_min_size(min, egui::vec2(w, area.height()));
                    draw_placeholder(ui, rect, tex);
                }
                continue;
            };
            // 右綴じ: 見開きは visible=[page, page+1] の page（1ページ目）を右半分、page+1 を左半分に置く。
            // 2 頁は窓の中央で接するよう、右の頁は半分の左端へ、左の頁は半分の右端へ寄せる
            let k = if n == 2 { 1 - k } else { k };
            let min = area.min + egui::vec2(w * k as f32, 0.0);
            let rect = egui::Rect::from_min_size(min, egui::vec2(w, area.height()));
            let align = match (n, k) {
                (2, 0) => egui::Align::Max,
                (2, _) => egui::Align::Min,
                _ => egui::Align::Center,
            };
            draw_slot(ui, rect, slot, align);
        }
        self.show_top_bar(ctx);
        if self.prompt.is_none() {
            self.show_page_slider(ctx, count);
        }
    }

    /// 上端にカーソルがある間だけ、頁番号・モード（全画面では冊名も）を頁の上に重ねて出す（#15）。
    /// 候補が無かったなどの通知はカーソルに関係なく出す（隠さない）。
    fn show_top_bar(&self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let near_top = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| screen.contains(p) && p.y <= screen.top() + TOP_ZONE);
        let mut lines = Vec::new();
        if near_top {
            lines.push((self.status.clone(), egui::Color32::WHITE));
        }
        if let Some(notice) = self.notice {
            lines.push((notice.to_string(), egui::Color32::YELLOW));
        }
        if lines.is_empty() {
            return;
        }
        // Area は初めて出るフレームを寸法取りに使って描かないので、通知が 1 フレーム遅れる。
        // 帯は最前面のレイヤーへ直接描く
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("top-bar"),
        ));
        let font = egui::TextStyle::Body.resolve(&ctx.global_style());
        let margin = egui::vec2(12.0, 6.0);
        let galleys: Vec<_> = lines
            .into_iter()
            .map(|(text, color)| painter.layout_no_wrap(text, font.clone(), color))
            .collect();
        let height = galleys.iter().map(|g| g.size().y).sum::<f32>() + margin.y * 2.0;
        let bar = egui::Rect::from_min_size(screen.left_top(), egui::vec2(screen.width(), height));
        painter.rect_filled(bar, 0.0, egui::Color32::from_black_alpha(190));
        let mut pos = bar.min + margin;
        for galley in galleys {
            let h = galley.size().y;
            painter.galley(pos, galley, egui::Color32::WHITE);
            pos.y += h;
        }
    }

    /// 上部に出す表示（頁番号・モード、全画面では冊名も）。上端にカーソルがあるときだけ見える。
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// 窓の下端にカーソルがある間（とつまんでいる間）だけ、頁スライダーを頁の上に重ねて出す。
    /// 右綴じなので右端が 1 頁目。つまんで動かす間は行き先の頁番号だけ出し、離したらそこへ飛ぶ
    /// （動かすたびに同期復号しないため）。トラックのクリックはその場で飛ぶ。
    fn show_page_slider(&mut self, ctx: &egui::Context, count: usize) {
        let screen = ctx.content_rect();
        let near_bottom = ctx
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| screen.contains(p) && p.y >= screen.bottom() - SLIDER_ZONE);
        if !near_bottom && self.slider_drag.is_none() {
            return;
        }
        let mut value = (self.slider_drag.unwrap_or(self.page) + 1) as f64;
        let shown = egui::Area::new(egui::Id::unique("page-slider"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.left(), screen.bottom() - SLIDER_HEIGHT))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(190))
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.set_width(screen.width() - 32.0);
                        ui.set_height(SLIDER_HEIGHT - 20.0);
                        ui.horizontal_centered(|ui| {
                            let digits = count.to_string().len();
                            ui.label(
                                egui::RichText::new(format!("{value:>digits$} / {count}"))
                                    .monospace()
                                    .color(egui::Color32::WHITE),
                            );
                            ui.add_space(12.0);
                            ui.spacing_mut().slider_width = ui.available_width();
                            // 範囲を大→小にして右端を 1 頁目にする
                            let max = count.max(1) as f64;
                            ui.add(
                                egui::Slider::new(&mut value, max..=1.0)
                                    .integer()
                                    .show_value(false),
                            )
                        })
                        .inner
                    })
                    .inner
            });
        self.slider_rect = Some(shown.response.rect);
        let resp = shown.inner;
        // 矢印キーはスライダーではなく頁送りに使うので、フォーカスは持たせない
        if resp.has_focus() {
            resp.surrender_focus();
        }
        let target = (value.round() as usize).clamp(1, count.max(1)) - 1;
        if resp.is_pointer_button_down_on() {
            self.slider_drag = Some(target);
        } else if self.slider_drag.take().is_some() || resp.changed() {
            self.jump_to(target);
        }
    }

    /// 頁 `page`（0 始まり）へ飛ぶ。見開きの組の先頭へは次のフレームの読み込みで揃う。
    fn jump_to(&mut self, page: usize) {
        if page != self.page {
            self.page = page;
            self.notice = None;
        }
    }

    /// 描画領域のどこでも押し下げ＋ドラッグで窓を動かす（状態遷移は [`crate::drag`]、優先順位もそこに明記）。
    /// 移動は OS 任せのドラッグ（`StartDrag`）で、閾値を超えた瞬間に 1 回だけ送る。
    fn update_window_drag(&mut self, ctx: &egui::Context) {
        // 全画面では窓を動かさない
        if is_fullscreen(ctx) {
            self.drag = Drag::default();
            return;
        }
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

    /// 全画面の切り替え（#14）。F11（一覧でも）か、閲覧中の頁の上のダブルクリックで切り替える。
    /// 全画面中の Esc は全画面を解くだけにする（一覧へ戻る・閉じるは窓のときだけ）。
    /// スライダーと次冊の確認の上のダブルクリックは対象外（前フレームのスライダーの範囲で判定）。
    fn handle_fullscreen(&mut self, ctx: &egui::Context) {
        let full = is_fullscreen(ctx);
        let f11 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F11));
        let esc =
            full && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let slider = self.slider_rect;
        let double = self.book.is_some()
            && self.prompt.is_none()
            && ctx.input(|i| {
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary)
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !slider.is_some_and(|r| r.contains(p)))
            });
        if f11 || double {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
        } else if esc {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
    }

    /// 確認中でないときのキー/ホイール入力（Esc 終了・F トグル・頁送り）。
    fn handle_input(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.library.is_some() {
                self.back_to_library();
                return;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(spread_toggle_pressed) {
            self.spread = !self.spread;
        }
        if ctx.input(shift_toggle_pressed) {
            self.toggle_shift();
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
        let layout = self.layout(ctx.content_rect().width());
        self.notice = None;
        match plan_nav(self.page, delta, layout, count) {
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
    fn ensure_loaded(&mut self, ctx: &egui::Context, layout: Layout) {
        let Some(book) = self.book.clone() else {
            return;
        };
        let count = book.page_count();
        // 窓が前に縮めた大きさより広がったら、粗く見えないよう作り直す（狭まる分は GPU の縮小で足りる）
        let target = window_target(ctx);
        self.target = target;
        if target.0 > self.decoded_for.0 || target.1 > self.decoded_for.1 {
            self.decoded_for = target;
            self.cache.clear();
            self.inflight.clear();
            self.generation += 1;
        }
        let target = self.decoded_for;
        // 見開きでは組の先頭に揃える（単頁から F で切り替えたときなど）
        let (start, end) = spread_pages(self.page, layout, count);
        self.page = start;
        self.visible = (start..=end).collect();
        // 表紙を仮表示している先頭頁は待たずに裏で復号し、届いたフレームで差し替える
        if self.page != 0 {
            self.placeholder = None;
        }
        let deferred = self.placeholder.is_some() && self.page == 0;
        if deferred && self.cache.contains_key(&0) {
            self.placeholder = None;
        }
        for i in self.visible.clone() {
            if deferred && i == 0 && self.placeholder.is_some() {
                self.drain(ctx, None);
                if self.cache.contains_key(&0) {
                    self.placeholder = None;
                } else {
                    self.spawn_load(ctx, &book, 0, target);
                }
                continue;
            }
            self.drain(ctx, Some(i));
            if !self.cache.contains_key(&i) {
                let (read_ms, res) = read_and_decode(&book, i, target);
                self.sync_loads += 1;
                self.cache.insert(i, upload(ctx, i, read_ms, res, "sync"));
            }
        }
        self.drain(ctx, None);
        for i in prefetch_plan(self.page, layout, count) {
            self.spawn_load(ctx, &book, i, target);
        }
        let keep = keep_range(self.page, count);
        self.cache.retain(|i, _| keep.contains(i));
    }

    /// 頁 `i` を裏で復号する（キャッシュ済み・復号中なら何もしない）。
    fn spawn_load(&mut self, ctx: &egui::Context, book: &Arc<Book>, i: usize, target: (u32, u32)) {
        if self.cache.contains_key(&i) || !self.inflight.insert(i) {
            return;
        }
        let (book, tx, ctx, generation) =
            (book.clone(), self.tx.clone(), ctx.clone(), self.generation);
        rayon::spawn(move || {
            let (read_ms, res) = read_and_decode(&book, i, target);
            let _ = tx.send((generation, i, read_ms, res));
            ctx.request_repaint();
        });
    }
}

/// 窓が全画面か（分からなければ窓扱い）。
fn is_fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen).unwrap_or(false)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// 窓の中身の大きさ（物理ピクセル）。頁はこれに収まるよう縮めてから持つ（#8）。
/// GPU のテクスチャの最大辺も超えない（超えると egui が拒む。巨大頁でも表示できるように）。
fn window_target(ctx: &egui::Context) -> (u32, u32) {
    let size = ctx.content_rect().size() * ctx.pixels_per_point();
    let max = ctx.input(|i| i.max_texture_side).max(1) as f32;
    let side = |v: f32| v.ceil().clamp(1.0, max) as u32;
    (side(size.x), side(size.y))
}

/// 頁のバイトを読んで `target` に収まるよう復号し、読み出し ms と結果を返す（復号 ms は stderr に出す）。
fn read_and_decode(book: &Book, i: usize, target: (u32, u32)) -> (f64, Result<Decoded, String>) {
    let t = Instant::now();
    let bytes = match book.read_page_bytes(i) {
        Ok(b) => b,
        Err(e) => return (ms(t), Err(e.to_string())),
    };
    let read_ms = ms(t);
    let t = Instant::now();
    let res = decode::decode_fit(&bytes, target.0, target.1).map_err(|e| e.to_string());
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
            slot.size = egui::vec2(img.source_size.0 as f32, img.source_size.1 as f32);
            slot.note = img.repair.map(decode::Repair::note);
            let color = color_image(size, img.rgba, img.opaque);
            let name = format!("page{i}");
            slot.texture = Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR));
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

/// RGBA を egui の画像にする。不透明なら非乗算 = 乗算済みなので、画素を変換せず
/// バッファをそのまま `Color32` として使う（#8: 全画素のコピーを 1 回減らす）。
fn color_image(size: [usize; 2], rgba: Vec<u8>, opaque: bool) -> egui::ColorImage {
    if opaque {
        match bytemuck::allocation::try_cast_vec::<u8, egui::Color32>(rgba) {
            Ok(pixels) => egui::ColorImage::new(size, pixels),
            Err((_, rgba)) => egui::ColorImage::from_rgba_premultiplied(size, &rgba),
        }
    } else {
        egui::ColorImage::from_rgba_unmultiplied(size, &rgba)
    }
}

/// 表紙テクスチャを `area` に収まるよう（拡大も許して）描く。
fn draw_placeholder(ui: &egui::Ui, area: egui::Rect, tex: &egui::TextureHandle) {
    let size = tex.size_vec2();
    let scale = (area.width() / size.x).min(area.height() / size.y);
    let rect = egui::Rect::from_center_size(area.center(), size * scale);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
}

/// 1 頁分を `area` の中に収めて描く（エラー/修復の注記は上、画像は残りの縦中央・横は `align` 寄せ）。
fn draw_slot(ui: &mut egui::Ui, area: egui::Rect, slot: &Slot, align: egui::Align) {
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
            let size = slot.size;
            let scale = (avail.width() / size.x)
                .min(avail.height() / size.y)
                .min(1.0);
            let rect = egui::Align2([align, egui::Align::Center])
                .align_size_within_rect(size * scale, avail);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
        }
    });
}

/// 開発用: `MV_SCREENSHOT=<フォルダ>` のとき、F12 で描いた画面をそのフォルダへ PNG で保存する。
/// OS の画面取り込みでは GL の中身が白く写るため、描画結果をアプリから直接取り出す。
fn save_screenshot_on_f12(ctx: &egui::Context) {
    let Some(dir) = std::env::var_os("MV_SCREENSHOT").map(PathBuf::from) else {
        return;
    };
    if !ctx.input(|i| i.key_pressed(egui::Key::F12)) {
        return;
    }
    ctx.request_screenshot(move |img| {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = dir.join(format!("shot-{stamp}.png"));
        let [w, h] = img.size;
        // 描画結果のアルファは 255 未満のことがあり、そのままだと画像ビューアで背景が透けて見えるので不透明にする
        let rgba = img
            .pixels
            .iter()
            .flat_map(|p| {
                let [r, g, b, _] = p.to_array();
                [r, g, b, 255]
            })
            .collect();
        let res = image::RgbaImage::from_raw(w as u32, h as u32, rgba).map(|buf| buf.save(&path));
        eprintln!("screenshot	{}	{res:?}", path.display());
    });
    ctx.request_repaint();
}

impl eframe::App for ViewApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Some(probe) = self.probe.as_mut() {
            probe.finish_previous(frame.info().cpu_usage);
        }
        self.show(ui);
        save_screenshot_on_f12(ui.ctx());
        let observed = self.observed();
        if let Some(probe) = self.probe.as_mut() {
            probe.observe(observed);
            if let Some(autopilot) = self.autopilot.as_mut() {
                autopilot.after_frame(ui.ctx(), probe);
            }
        }
    }

    /// 見開きの希望を保存する（窓の大きさ・位置は eframe が保存する）。持ち越さない窓では何もしない。
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.persist {
            storage.set_string(SPREAD_KEY, if self.spread { "1" } else { "0" }.to_string());
            let mut books: Vec<String> = self
                .shifted_books
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            books.sort();
            storage.set_string(SHIFTED_KEY, books.join("\n"));
        }
    }

    /// egui 内部の状態（ウィジェットの開閉など）は使わないので保存しない。
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let now = Instant::now();
        if let Some(probe) = self.probe.as_mut() {
            probe.begin_frame(now);
        }
        if let Some(autopilot) = self.autopilot.as_mut() {
            autopilot.inject(now, &mut raw_input.events);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // MV_TRACE の普段使いは、閉じるときに要約を出す（autopilot は自分で出す）
        if let (Some(probe), None) = (&self.probe, &self.autopilot) {
            for (name, n, p95, max) in probe.summary() {
                eprintln!("trace	{name}	n={n}	p95={p95:.1}	max={max:.1}");
            }
            if let Some(rss) = crate::autopilot::rss_mb() {
                eprintln!("trace	rss_mb	{rss:.0}");
            }
        }
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

/// 日本語フォントの字面を下げる量（文字の大きさに対する比）。
const JP_FONT_Y_OFFSET: f32 = 0.33;

fn register_font(ctx: &egui::Context, data: Vec<u8>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "jp".into(),
        std::sync::Arc::new(egui::FontData::from_owned(data).tweak(egui::FontTweak {
            // 游ゴシック等は行の箱の中で字面が上に寄る（ボタン中央より上に見える）ので下げる
            y_offset_factor: JP_FONT_Y_OFFSET,
            ..Default::default()
        })),
    );
    // 文章（Proportional）は日本語フォントを先頭に置き、英数字も同じフォントで描く。
    // 末尾の代替のままだと英数字は既定フォント・かなは日本語フォントになり、行の中で高さがずれる
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "jp".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("jp".into());
    ctx.set_fonts(fonts);
}

/// 引数なし起動（#1）。索引 DB があって冊が入っていれば一覧、無ければ空の窓（ドロップ待ち）。
/// 一覧のときは表紙の無い冊の表紙を裏で作り始める。DB が無いときに空の DB を作らない。
///
/// `t0` は `main` の先頭の時刻。`MV_TRACE=1` なら起動・冊を開く・頁送りの所要を stderr に出し（#3）、
/// `autopilot` があればその場面を自動で再現して `BENCH` 行を stdout に出して閉じる。
pub fn run_start(
    db: &Path,
    root: Option<&Path>,
    t0: Instant,
    autopilot: Option<Mode>,
) -> anyhow::Result<()> {
    let trace = std::env::var_os("MV_TRACE").is_some_and(|v| v == "1");
    if db.exists() {
        let library = Library::open(db, root)?;
        if !library.rows().is_empty() {
            library.spawn_cover_generation();
            let mut app = ViewApp::with_library(library);
            if trace || autopilot.is_some() {
                app = app.with_probe(Probe::new(t0, trace), autopilot.map(Autopilot::new));
            }
            return launch(app);
        }
    }
    if autopilot.is_some() {
        return Err(anyhow!(
            "--autopilot には冊の入った索引 DB が要ります: {}",
            db.display()
        ));
    }
    run_empty(None)
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
/// 頁の無い冊は 0 頁目扱いで、窓は開く。
pub fn run(
    title: String,
    book: Book,
    page: usize,
    exit_after: Option<Duration>,
) -> anyhow::Result<()> {
    launch(ViewApp::new(title, book, page, exit_after))
}

/// 組み立て済みの [`ViewApp`] を窓で動かす。窓が閉じるまで戻らない。
pub fn launch(mut app: ViewApp) -> anyhow::Result<()> {
    app.persist = app.persists();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app.window_title())
            .with_inner_size([900.0, 1000.0]),
        // vsync を切る（#3）。この機の実窓の測定で、vsync 有りは swap が 1 フレームにつき 31〜46 ms
        // 止まり、冊を開いて 1 頁目が出るまで 96 ms かかった（切ると 24 ms）。Windows の窓表示は
        // DWM が vblank で合成するので切っても裂けない。egui は入力や要求があるときだけ描くので待機中は回らない。
        // `MV_VSYNC=1` で戻せる
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: std::env::var_os("MV_VSYNC").is_some_and(|v| v == "1"),
            ..Default::default()
        },
        // 窓の大きさ・位置・最大化・全画面は eframe が保存して戻す（#13）。画面外に出る位置は
        // eframe（egui-winit）が今のモニターへ寄せる。保存先は索引 DB と同じフォルダ
        persist_window: app.persist,
        persistence_path: crate::index::default_db_path()
            .ok()
            .and_then(|db| Some(db.parent()?.join(STATE_FILE))),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            install_japanese_font(&cc.egui_ctx);
            if let (true, Some(storage)) = (app.persist, cc.storage) {
                app.restore_state(storage);
            }
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
