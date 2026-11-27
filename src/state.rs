use std::collections::VecDeque;
use std::time::Instant;

use crate::config::{Config, PingColors, WaveSpeed};
use crate::ping::{PingResult, PingTier};
use crate::ui::win_util::{lerp_color, rgb, BG_MAIN};

#[derive(Clone, Debug)]
pub struct HistoryCell {
    pub text: String,
    pub tier: PingTier,
    pub anim_start: Option<Instant>,
}

pub struct TargetStats {
    pub target: String,
    pub loss_window: VecDeque<bool>,
    pub avg_window: VecDeque<u32>,
    pub history: VecDeque<HistoryCell>,
    loss_max: usize,
    avg_max: usize,
    history_max: usize,
}

impl TargetStats {
    pub fn new(target: String, cfg: &Config) -> Self {
        let mut history = VecDeque::with_capacity(cfg.history_count);
        for _ in 0..cfg.history_count {
            history.push_back(HistoryCell {
                text: "...".to_string(),
                tier: PingTier::Empty,
                anim_start: None,
            });
        }
        Self {
            target,
            loss_window: VecDeque::with_capacity(cfg.loss_window_size),
            avg_window: VecDeque::with_capacity(cfg.avg_window_size),
            history,
            loss_max: cfg.loss_window_size,
            avg_max: cfg.avg_window_size,
            history_max: cfg.history_count,
        }
    }

    pub fn push(&mut self, res: PingResult, wave_enabled: bool) {
        let now = Instant::now();

        if self.loss_window.len() == self.loss_max {
            self.loss_window.pop_front();
        }
        self.loss_window.push_back(res.is_lost);

        if let Some(ms) = res.latency_ms {
            if self.avg_window.len() == self.avg_max {
                self.avg_window.pop_front();
            }
            self.avg_window.push_back(ms);
        }

        let old_texts: Vec<String> = self.history.iter().map(|c| c.text.clone()).collect();

        if self.history.len() == self.history_max {
            self.history.pop_back();
        }
        self.history.push_front(HistoryCell {
            text: res.display,
            tier: res.tier,
            anim_start: None,
        });

        if wave_enabled {
            for (idx, cell) in self.history.iter_mut().enumerate() {
                if old_texts.get(idx).map(|s| s.as_str()) != Some(cell.text.as_str()) {
                    cell.anim_start = Some(now);
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.loss_window.clear();
        self.avg_window.clear();
        for cell in &mut self.history {
            cell.text = "...".to_string();
            cell.tier = PingTier::Empty;
            cell.anim_start = None;
        }
    }

    pub fn loss_pct(&self) -> f64 {
        if self.loss_window.is_empty() {
            return 0.0;
        }
        let lost = self.loss_window.iter().filter(|&&b| b).count();
        (lost as f64 / self.loss_window.len() as f64) * 100.0
    }

    pub fn avg_ms(&self) -> u32 {
        if self.avg_window.is_empty() {
            return 0;
        }
        let sum: u64 = self.avg_window.iter().map(|&v| v as u64).sum();
        (sum / self.avg_window.len() as u64) as u32
    }

    pub fn tier_color(tier: PingTier, colors: &PingColors) -> u32 {
        match tier {
            PingTier::Empty => rgb(0x44, 0x44, 0x44),
            PingTier::Low => colors.low,
            PingTier::Medium => colors.medium,
            PingTier::High => colors.high,
            PingTier::Timeout => colors.timeout,
        }
    }

    pub fn loss_color(&self, colors: &PingColors) -> u32 {
        let pct = self.loss_pct();
        if pct > 10.0 {
            colors.timeout
        } else if pct > 0.0 {
            colors.high
        } else {
            rgb(0x77, 0x77, 0x77)
        }
    }

    pub fn avg_color(&self, colors: &PingColors) -> u32 {
        let avg = self.avg_ms();
        if avg < 100 {
            colors.low
        } else if avg <= 200 {
            colors.medium
        } else {
            colors.high
        }
    }

    pub fn cell_render_color(
        &self,
        idx: usize,
        wave_speed: WaveSpeed,
        colors: &PingColors,
        now: Instant,
    ) -> (u32, bool) {
        let cell = &self.history[idx];
        let target_color = Self::tier_color(cell.tier, colors);
        let Some(start) = cell.anim_start else {
            return (target_color, false);
        };

        let (cascade_ms, fade_ms) = wave_speed.timing_ms();
        let delay = (idx as u64) * cascade_ms;
        let elapsed = now.saturating_duration_since(start).as_millis() as u64;

        if elapsed < delay {
            (BG_MAIN, true)
        } else if elapsed < delay + fade_ms {
            let t = (elapsed - delay) as f32 / fade_ms.max(1) as f32;
            (lerp_color(BG_MAIN, target_color, t), true)
        } else {
            (target_color, false)
        }
    }
}

pub struct AppStats {
    pub rows: Vec<TargetStats>,
}

impl AppStats {
    pub fn from_config(cfg: &Config) -> Self {
        let rows = cfg
            .targets
            .iter()
            .map(|t| TargetStats::new(t.clone(), cfg))
            .collect();
        Self { rows }
    }

    pub fn reset(&mut self) {
        for r in &mut self.rows {
            r.reset();
        }
    }

    pub fn has_active_animations(&mut self, wave_speed: WaveSpeed, now: Instant) -> bool {
        let (cascade_ms, fade_ms) = wave_speed.timing_ms();
        let mut active = false;
        for row in &mut self.rows {
            for (idx, cell) in row.history.iter_mut().enumerate() {
                if let Some(start) = cell.anim_start {
                    let total = (idx as u64) * cascade_ms + fade_ms;
                    if now.saturating_duration_since(start).as_millis() as u64 >= total {
                        cell.anim_start = None;
                    } else {
                        active = true;
                    }
                }
            }
        }
        active
    }

    pub fn build_window_title(&self) -> String {
        if self.rows.is_empty() {
            return "NanoPing".to_string();
        }
        let parts: Vec<String> = self
            .rows
            .iter()
            .map(|r| {
                let loss = r.loss_pct().round() as u32;
                let val = if r.avg_window.is_empty() {
                    if r.loss_window.is_empty() {
                        "...".to_string()
                    } else {
                        "T/O".to_string()
                    }
                } else {
                    r.avg_ms().to_string()
                };
                if loss > 0 {
                    format!("{val} (L:{loss})")
                } else {
                    val
                }
            })
            .collect();
        format!("NanoPing \u{00B7} {}", parts.join(" | "))
    }
}