//! 窓のドラッグ移動の状態遷移（純粋ロジック）。egui の入力には依存しない。
//!
//! 押し下げ → 閾値（[`DRAG_THRESHOLD`] 点）超過で移動モード → 離すと終了。
//! 閾値未満で離したら単なるクリックで、移動は起きない。
//!
//! 優先順位（view.rs が従う）:
//! - 移動モード中は頁送り（キー/ホイール）を無視する。閾値に届くまでの押し下げ中は通常どおり。
//! - 押し下げ位置がボタンなど egui のウィジェット上なら移動を始めない（ウィジェット優先）。
//! - 閲覧中の頁の上のダブルクリックは全画面の切り替え（#14、view.rs）。押し下げが閾値内で終わるので移動は起きない。
//!   全画面中は移動しない。クリックでの頁送りは無い。

/// 移動モードに入るまでの押し下げ位置からの距離（egui の点）。
pub const DRAG_THRESHOLD: f32 = 6.0;

/// ドラッグの状態。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Drag {
    #[default]
    Idle,
    /// 押し下げ中（まだ閾値内）。押した位置を持つ
    Pressed { origin: (f32, f32) },
    /// 移動モード
    Moving,
}

/// 1 フレームの入力。
#[derive(Clone, Copy, Debug)]
pub struct DragInput {
    /// 主ボタンがこのフレームで押された
    pub pressed: bool,
    /// 主ボタンが今押されている
    pub down: bool,
    /// ポインタ位置（窓内、無ければ `None`）
    pub pos: Option<(f32, f32)>,
    /// 押した位置がウィジェット上（移動を始めない）
    pub on_widget: bool,
}

impl Drag {
    /// 1 フレーム進める。戻りの `true` は「このフレームで移動モードに入った」（窓移動を 1 回始める合図）。
    pub fn update(&mut self, input: DragInput) -> bool {
        if !input.down {
            *self = Drag::Idle;
            return false;
        }
        match *self {
            Drag::Idle => {
                if input.pressed
                    && !input.on_widget
                    && let Some(origin) = input.pos
                {
                    *self = Drag::Pressed { origin };
                }
                false
            }
            Drag::Pressed { origin } => {
                // 動き出す前にウィジェット（スクロールバーなど）がポインタを取ったら、窓は動かさない
                if input.on_widget {
                    *self = Drag::Idle;
                    return false;
                }
                let Some((x, y)) = input.pos else {
                    return false;
                };
                if (x - origin.0).hypot(y - origin.1) > DRAG_THRESHOLD {
                    *self = Drag::Moving;
                    true
                } else {
                    false
                }
            }
            Drag::Moving => false,
        }
    }

    /// 移動モード中か（頁送りを抑止する判定）。
    #[must_use]
    pub fn is_moving(self) -> bool {
        self == Drag::Moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(pressed: bool, down: bool, pos: (f32, f32)) -> DragInput {
        DragInput {
            pressed,
            down,
            pos: Some(pos),
            on_widget: false,
        }
    }

    #[test]
    fn press_then_small_move_is_not_a_drag() {
        let mut d = Drag::default();
        assert!(!d.update(inp(true, true, (10.0, 10.0))));
        assert!(!d.update(inp(false, true, (13.0, 12.0))));
        assert!(!d.is_moving());
        assert!(!d.update(inp(false, false, (13.0, 12.0))));
        assert_eq!(d, Drag::Idle);
    }

    #[test]
    fn crossing_threshold_starts_move_once_and_release_ends() {
        let mut d = Drag::default();
        d.update(inp(true, true, (0.0, 0.0)));
        assert!(d.update(inp(false, true, (20.0, 0.0))));
        assert!(d.is_moving());
        assert!(
            !d.update(inp(false, true, (40.0, 0.0))),
            "開始の合図は 1 回"
        );
        assert!(d.is_moving());
        d.update(inp(false, false, (40.0, 0.0)));
        assert!(!d.is_moving());
    }

    #[test]
    fn threshold_is_exclusive() {
        let mut d = Drag::default();
        d.update(inp(true, true, (0.0, 0.0)));
        assert!(!d.update(inp(false, true, (DRAG_THRESHOLD, 0.0))));
        assert!(d.update(inp(false, true, (DRAG_THRESHOLD + 0.1, 0.0))));
    }

    #[test]
    fn press_on_widget_never_moves() {
        let mut d = Drag::default();
        let mut i = inp(true, true, (0.0, 0.0));
        i.on_widget = true;
        d.update(i);
        assert!(!d.update(inp(false, true, (100.0, 100.0))));
        assert_eq!(d, Drag::Idle);
    }

    #[test]
    fn widget_taking_pointer_before_threshold_cancels() {
        // 一覧のスクロールバーなど、押した後でドラッグを掴んだウィジェットがあれば動かさない
        let mut d = Drag::default();
        d.update(inp(true, true, (0.0, 0.0)));
        let mut i = inp(false, true, (20.0, 0.0));
        i.on_widget = true;
        assert!(!d.update(i));
        assert!(!d.update(inp(false, true, (40.0, 0.0))));
        assert_eq!(d, Drag::Idle);
    }

    #[test]
    fn held_without_press_edge_does_not_start() {
        // 窓外から押したまま入ってきた等、押し下げ瞬間を見ていないときは始めない
        let mut d = Drag::default();
        assert!(!d.update(inp(false, true, (0.0, 0.0))));
        assert!(!d.update(inp(false, true, (50.0, 0.0))));
        assert_eq!(d, Drag::Idle);
    }

    #[test]
    fn release_mid_press_resets_and_next_press_restarts() {
        let mut d = Drag::default();
        d.update(inp(true, true, (0.0, 0.0)));
        d.update(inp(false, false, (0.0, 0.0)));
        assert_eq!(d, Drag::Idle);
        d.update(inp(true, true, (100.0, 100.0)));
        assert!(d.update(inp(false, true, (120.0, 100.0))));
    }

    #[test]
    fn pointer_lost_while_pressed_waits() {
        let mut d = Drag::default();
        d.update(inp(true, true, (0.0, 0.0)));
        let mut i = inp(false, true, (0.0, 0.0));
        i.pos = None;
        assert!(!d.update(i));
        assert!(matches!(d, Drag::Pressed { .. }));
    }
}
