//! Container resource statistics: samples, derived percentages and history.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bollard::models::ContainerStatsResponse;

pub const HISTORY: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatsSample {
    pub cpu_total: u64,
    pub precpu_total: u64,
    pub system: u64,
    pub presystem: u64,
    pub online_cpus: u32,
    pub mem_usage: u64,
    pub mem_limit: u64,
    pub pids: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl StatsSample {
    pub fn from_bollard(s: &ContainerStatsResponse) -> Self {
        let cpu = s.cpu_stats.as_ref();
        let pre = s.precpu_stats.as_ref();
        let (rx, tx) = s
            .networks
            .as_ref()
            .map(|n| {
                n.values().fold((0, 0), |(r, t), v| {
                    (r + v.rx_bytes.unwrap_or(0), t + v.tx_bytes.unwrap_or(0))
                })
            })
            .unwrap_or((0, 0));
        StatsSample {
            cpu_total: cpu
                .and_then(|c| c.cpu_usage.as_ref())
                .and_then(|u| u.total_usage)
                .unwrap_or(0),
            precpu_total: pre
                .and_then(|c| c.cpu_usage.as_ref())
                .and_then(|u| u.total_usage)
                .unwrap_or(0),
            system: cpu.and_then(|c| c.system_cpu_usage).unwrap_or(0),
            presystem: pre.and_then(|c| c.system_cpu_usage).unwrap_or(0),
            online_cpus: cpu
                .and_then(|c| {
                    c.online_cpus.or_else(|| {
                        c.cpu_usage
                            .as_ref()
                            .and_then(|u| u.percpu_usage.as_ref())
                            .map(|p| p.len() as u32)
                    })
                })
                .unwrap_or(1),
            mem_usage: s.memory_stats.as_ref().and_then(|m| m.usage).unwrap_or(0),
            mem_limit: s.memory_stats.as_ref().and_then(|m| m.limit).unwrap_or(0),
            pids: s.pids_stats.as_ref().and_then(|p| p.current).unwrap_or(0),
            rx_bytes: rx,
            tx_bytes: tx,
        }
    }

    /// docker CLI formula: Δcontainer / Δsystem × online CPUs × 100.
    pub fn cpu_percent(&self) -> f64 {
        let dc = self.cpu_total.saturating_sub(self.precpu_total) as f64;
        let ds = self.system.saturating_sub(self.presystem) as f64;
        if ds <= 0.0 || dc <= 0.0 {
            return 0.0;
        }
        dc / ds * self.online_cpus.max(1) as f64 * 100.0
    }

    pub fn mem_percent(&self) -> f64 {
        if self.mem_limit == 0 {
            0.0
        } else {
            self.mem_usage as f64 * 100.0 / self.mem_limit as f64
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatsHistory {
    pub samples: VecDeque<(Instant, StatsSample)>,
}

impl StatsHistory {
    pub fn push(&mut self, at: Instant, s: StatsSample) {
        self.samples.push_back((at, s));
        while let Some((t, _)) = self.samples.front() {
            if at.duration_since(*t) > HISTORY {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn latest(&self) -> Option<&StatsSample> {
        self.samples.back().map(|(_, s)| s)
    }

    pub fn span(&self) -> Duration {
        match (self.samples.front(), self.samples.back()) {
            (Some((a, _)), Some((b, _))) => b.duration_since(*a),
            _ => Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_formula() {
        let s = StatsSample {
            cpu_total: 300,
            precpu_total: 100,
            system: 2100,
            presystem: 100,
            online_cpus: 4,
            ..Default::default()
        };
        assert!((s.cpu_percent() - 40.0).abs() < 1e-9);
        let zero = StatsSample {
            system: 5,
            presystem: 5,
            ..Default::default()
        };
        assert_eq!(zero.cpu_percent(), 0.0);
        let mem = StatsSample {
            mem_usage: 25,
            mem_limit: 100,
            ..Default::default()
        };
        assert_eq!(mem.mem_percent(), 25.0);
    }

    #[test]
    fn stats_history_trimmed() {
        let mut h = StatsHistory::default();
        let t0 = Instant::now();
        h.push(t0, StatsSample::default());
        h.push(t0 + Duration::from_secs(100), StatsSample::default());
        h.push(
            t0 + Duration::from_secs(200),
            StatsSample {
                pids: 7,
                ..Default::default()
            },
        );
        assert_eq!(h.samples.len(), 2);
        assert_eq!(h.latest().unwrap().pids, 7);
        assert_eq!(h.span(), Duration::from_secs(100));
    }
}
