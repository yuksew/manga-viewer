//! 実窓での通し測定（#3、設計 §2.1 / p0-3c）。
//!
//! * [`Probe`] — 「待ちの始まり → その絵が載ったフレームの終わり」を測る。`MV_TRACE=1` の普段使いでも使う
//! * [`Autopilot`] — キー入力を注入して B-2/B-4/B-1/B-1'/B-5 の場面を順に再現し、`BENCH` 行を出して窓を閉じる
//!
//! フレームの終わりは eframe の `cpu_usage` で取る（前フレームの値が次のフレームで読める）。
//! GPU へのテクスチャ転送（`paint_and_update_textures`）を含み、vsync 待ちの swap は含まない。

use std::time::{Duration, Instant};

use eframe::egui;

/// `--autopilot` の場面。
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// 起動 → 一覧の初回フレーム → 見えている表紙がそろうまで（B-2）。そろったら閉じる
    Startup,
    /// startup の後、先頭の冊を開いて（B-4）頁を送る（B-1 / B-1'）。最後に RSS（B-5）
    Reader,
}

/// フレームごとに見る閲覧側の状態（`show` の後に取る）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    pub in_library: bool,
    pub covers_settled: bool,
    /// 冊を開いた回数（開くたびに増える）
    pub opens: u64,
    pub page: usize,
    /// 表示中の頁がすべてキャッシュにあり、このフレームに頁の絵が載る
    pub page_ready: bool,
    /// 先頭頁の代わりに表紙を仮表示している
    pub placeholder: bool,
}

/// 待っている物。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    List,
    Covers,
    Placeholder,
    Page,
    Turn,
}

/// 待ちの始まりから、その絵が載ったフレームの終わりまでを測る。
pub struct Probe {
    t0: Instant,
    frame_start: Instant,
    prev: Option<Observed>,
    waits: Vec<(Wait, Instant)>,
    /// 載ったフレームの始まりまでの ms。そのフレームの所要（次フレームで分かる）を足して標本にする
    landed: Vec<(&'static str, f64)>,
    samples: Vec<(&'static str, f64)>,
    /// 載る前に次の送りで追い越された頁送りの数
    overtaken: usize,
    /// 頁送りの標本の名前（autopilot が場面ごとに替える）
    pub turn_label: &'static str,
    /// 標本ができるたびに stderr へ 1 行出す（`MV_TRACE=1`）
    print: bool,
}

impl Probe {
    /// `t0` は `main` の先頭の時刻（起動の測定の起点）。
    #[must_use]
    pub fn new(t0: Instant, print: bool) -> Self {
        Self {
            t0,
            frame_start: t0,
            prev: None,
            waits: vec![(Wait::List, t0), (Wait::Covers, t0)],
            landed: Vec::new(),
            samples: Vec::new(),
            overtaken: 0,
            turn_label: "turn",
            print,
        }
    }

    /// フレームの始まり（`raw_input_hook`）。注入した入力の時刻もこれ。
    pub fn begin_frame(&mut self, now: Instant) {
        self.frame_start = now;
    }

    /// 前フレームの所要（`cpu_usage` 秒）で、前フレームに載った物の標本を確定する。
    pub fn finish_previous(&mut self, cpu_usage: Option<f32>) {
        let frame_ms = cpu_usage.map_or(0.0, |s| f64::from(s) * 1000.0);
        for (name, before) in std::mem::take(&mut self.landed) {
            let v = before + frame_ms;
            if self.print {
                eprintln!("trace\t{name}_ms\t{v:.1}");
            }
            self.samples.push((name, v));
        }
    }

    /// このフレームの状態から、始まった待ち・載った物を記録する。
    pub fn observe(&mut self, o: Observed) {
        let now = self.frame_start;
        if let Some(prev) = self.prev {
            if o.opens != prev.opens {
                // 開き直したら前の冊の待ちは捨てる
                self.waits
                    .retain(|(w, _)| !matches!(w, Wait::Placeholder | Wait::Page | Wait::Turn));
                self.waits.push((Wait::Placeholder, now));
                self.waits.push((Wait::Page, now));
            } else if o.page != prev.page && !o.in_library {
                if let Some(i) = self.waits.iter().position(|(w, _)| *w == Wait::Turn) {
                    self.waits.remove(i);
                    self.overtaken += 1;
                }
                self.waits.push((Wait::Turn, now));
            }
        }
        self.prev = Some(o);
        let turn = self.turn_label;
        let waits = std::mem::take(&mut self.waits);
        for (wait, start) in waits {
            let name = match wait {
                Wait::List if o.in_library => Some("startup_list"),
                Wait::Covers if o.in_library && o.covers_settled => Some("startup_covers"),
                Wait::Placeholder if o.placeholder => Some("open_placeholder"),
                // 仮表示の無い開き方（索引から直接・ドロップ）では先頭頁が仮表示より先に載る
                Wait::Placeholder if o.page_ready && !o.in_library => None,
                Wait::Page | Wait::Turn if o.page_ready && !o.placeholder && !o.in_library => {
                    Some(if wait == Wait::Page {
                        "open_page"
                    } else {
                        turn
                    })
                }
                _ => {
                    self.waits.push((wait, start));
                    continue;
                }
            };
            if let Some(name) = name {
                let before = now.duration_since(start).as_secs_f64() * 1000.0;
                self.landed.push((name, before));
            }
        }
    }

    /// `name` の標本。
    #[must_use]
    pub fn samples(&self, name: &str) -> Vec<f64> {
        self.samples
            .iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .collect()
    }

    /// 載る前に追い越された頁送りの数。
    #[must_use]
    pub fn overtaken(&self) -> usize {
        self.overtaken
    }

    /// 標本の名前ごとの件数・p95・max（出た順）。
    #[must_use]
    pub fn summary(&self) -> Vec<(&'static str, usize, f64, f64)> {
        let mut names: Vec<&'static str> = Vec::new();
        for (n, _) in &self.samples {
            if !names.contains(n) {
                names.push(n);
            }
        }
        names
            .into_iter()
            .map(|n| {
                let v = self.samples(n);
                let (p95, max) = p95_max(&v);
                (n, v.len(), p95, max)
            })
            .collect()
    }

    /// 起動からの経過。
    #[must_use]
    pub fn since_start(&self) -> Duration {
        self.t0.elapsed()
    }
}

/// p95 と max（空なら 0）。
#[must_use]
pub fn p95_max(v: &[f64]) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    let i = ((v.len() as f64 * 0.95).ceil() as usize).clamp(1, v.len()) - 1;
    (v[i], v[v.len() - 1])
}

/// 今のプロセスの物理メモリ（MB）。取れなければ `None`。
#[must_use]
pub fn rss_mb() -> Option<f64> {
    memory_stats::memory_stats().map(|m| m.physical_mem as f64 / (1024.0 * 1024.0))
}

/// 先読み命中の頁送りの回数と間隔（設計 B-1: 100 ms 間隔で 100 頁）。
pub const HIT_TURNS: usize = 100;
const HIT_INTERVAL: Duration = Duration::from_millis(100);
/// 先読みを追い越す頁送り（毎フレーム 1 回、逆向き）の回数。
pub const MISS_TURNS: usize = 60;
/// 冊を開いてから先読みが落ち着くまで待つ時間。
const SETTLE: Duration = Duration::from_millis(500);
/// 何かが止まっても窓を残さないための上限。
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    WaitCovers,
    Open,
    WaitPage,
    Settle(Instant),
    Hit { sent: usize, next: Instant },
    Miss { sent: usize },
    Finish,
    Done,
}

/// 場面を順に再現する（入力の注入は [`Autopilot::inject`]、進行は [`Autopilot::after_frame`]）。
pub struct Autopilot {
    mode: Mode,
    step: Step,
    started: Instant,
    rss_after_hit: Option<f64>,
}

impl Autopilot {
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            step: Step::WaitCovers,
            started: Instant::now(),
            rss_after_hit: None,
        }
    }

    /// このフレームに入れるキー（`raw_input_hook` から）。
    pub fn inject(&mut self, now: Instant, events: &mut Vec<egui::Event>) {
        let key = match &mut self.step {
            Step::Open => {
                self.step = Step::WaitPage;
                Some(egui::Key::Enter)
            }
            Step::Hit { sent, next } if *sent < HIT_TURNS && now >= *next => {
                *sent += 1;
                *next += HIT_INTERVAL;
                Some(egui::Key::ArrowRight)
            }
            Step::Miss { sent } if *sent < MISS_TURNS => {
                *sent += 1;
                Some(egui::Key::ArrowLeft)
            }
            _ => None,
        };
        if let Some(key) = key {
            for pressed in [true, false] {
                events.push(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
    }

    /// フレームの後に進め、次に起きる時刻を頼む。終わったら `BENCH` 行を出して窓を閉じる。
    pub fn after_frame(&mut self, ctx: &egui::Context, probe: &mut Probe) {
        let now = Instant::now();
        if self.started.elapsed() > TIMEOUT && self.step != Step::Done {
            eprintln!("BENCH autopilot_timeout_step {:?}", self.step);
            self.step = Step::Finish;
        }
        let has = |name: &str, n: usize| probe.samples(name).len() >= n;
        self.step = match self.step {
            Step::WaitCovers if has("startup_covers", 1) => match self.mode {
                Mode::Startup => Step::Finish,
                Mode::Reader => Step::Open,
            },
            Step::WaitPage if has("open_page", 1) => Step::Settle(now + SETTLE),
            Step::Settle(at) if now >= at => {
                probe.turn_label = "b1_hit";
                Step::Hit { sent: 0, next: now }
            }
            Step::Hit { sent, .. } if sent >= HIT_TURNS && has("b1_hit", HIT_TURNS) => {
                self.rss_after_hit = rss_mb();
                probe.turn_label = "b1_miss";
                Step::Miss { sent: 0 }
            }
            Step::Miss { sent } if sent >= MISS_TURNS && probe.landed.is_empty() => Step::Finish,
            Step::Finish => {
                self.report(probe);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                Step::Done
            }
            step => step,
        };
        match self.step {
            Step::Hit { next, .. } => {
                ctx.request_repaint_after(next.saturating_duration_since(now))
            }
            Step::Settle(at) => ctx.request_repaint_after(at.saturating_duration_since(now)),
            Step::Done => {}
            // vsync を切っているので、すぐの再描画だと空回りして裏の復号の CPU を奪う
            _ => ctx.request_repaint_after(Duration::from_millis(1)),
        }
    }

    fn report(&self, probe: &Probe) {
        for (name, n, p95, max) in probe.summary() {
            match n {
                1 => println!("BENCH {name}_ms {max:.1}"),
                _ => println!("BENCH {name}_ms n={n} p95={p95:.1} max={max:.1}"),
            }
        }
        println!("BENCH overtaken {}", probe.overtaken());
        if let Some(rss) = self.rss_after_hit {
            println!("BENCH b5_rss_after_{HIT_TURNS}_turns_mb {rss:.0}");
        }
        if let Some(rss) = rss_mb() {
            println!("BENCH rss_at_exit_mb {rss:.0}");
        }
        println!(
            "BENCH autopilot_total_ms {:.0}",
            probe.since_start().as_secs_f64() * 1000.0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(page: usize, ready: bool) -> Observed {
        Observed {
            opens: 1,
            page,
            page_ready: ready,
            ..Observed::default()
        }
    }

    #[test]
    fn turn_lands_with_the_frame_that_drew_it() {
        // 実際の順: begin_frame → finish_previous（前フレームの所要）→ show → observe
        let t0 = Instant::now();
        let mut p = Probe::new(t0, false);
        let frame = |p: &mut Probe, at: u64, prev_cpu: f32, o: Observed| {
            p.begin_frame(t0 + Duration::from_millis(at));
            p.finish_previous(Some(prev_cpu));
            p.observe(o);
        };
        frame(&mut p, 0, 0.0, obs(0, true));
        // 送った頁はまだ無い → 16 ms 後のフレームで載る（そのフレームの所要 5 ms は次で分かる）
        frame(&mut p, 10, 0.004, obs(1, false));
        frame(&mut p, 26, 0.002, obs(1, true));
        assert!(p.samples("turn").is_empty());
        frame(&mut p, 42, 0.005, obs(1, true));
        let v = p.samples("turn");
        assert_eq!(v.len(), 1);
        assert!((v[0] - 21.0).abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn overtaken_turn_is_counted_not_sampled() {
        let t0 = Instant::now();
        let mut p = Probe::new(t0, false);
        p.observe(obs(5, true));
        p.observe(obs(4, false));
        p.observe(obs(3, true));
        p.finish_previous(Some(0.001));
        assert_eq!(p.samples("turn").len(), 1);
        assert_eq!(p.overtaken(), 1);
    }

    #[test]
    fn p95_of_twenty_is_the_nineteenth() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(p95_max(&v), (19.0, 20.0));
        assert_eq!(p95_max(&[]), (0.0, 0.0));
    }
}
