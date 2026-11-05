//! A week of account quota. Providers only report the current percentage, so agtk keeps its
//! own readings to show the week so far and project the rest.

use super::quota::AccountQuota;
use crate::persist::Store;
use crate::providers::AgentProvider;

const WEEK: u64 = 7 * 24 * 60 * 60;
/// One reading per bucket is still finer than a pixel of the week's graph.
const BUCKET: u64 = 5 * 60;
/// Readings further apart than this say nothing about the time between them:
/// agtk wasn't running, or the provider couldn't be read.
const GAP: u64 = 30 * 60;
/// Before this much of the week has passed, the pace says too little about the rest.
const MIN_PACE: u64 = 6 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuotaSample {
    pub at: u64,
    pub used: f64,
}

/// Where the week is heading if the pace so far holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outlook {
    TooEarly,
    LimitReached,
    AtReset(f64),
    RunsOut(u64),
}

/// What is known about one moment of the week.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    Recorded(f64),
    NotRecorded,
    Projected(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWeek {
    pub provider: AgentProvider,
    pub start: u64,
    pub end: u64,
    /// The latest reading, where the recorded line stops and the projection starts.
    pub latest: QuotaSample,
    line: Vec<QuotaSample>,
}

impl QuotaWeek {
    pub fn new(
        provider: AgentProvider,
        end: u64,
        latest: QuotaSample,
        samples: impl IntoIterator<Item = QuotaSample>,
    ) -> Self {
        let start = end.saturating_sub(WEEK);
        // A week's quota starts unused, whether or not agtk saw that moment.
        let mut line = vec![QuotaSample {
            at: start,
            used: 0.0,
        }];
        line.extend(
            samples
                .into_iter()
                .filter(|sample| sample.at > start && sample.at < latest.at),
        );
        line.push(latest);
        Self {
            provider,
            start,
            end,
            latest,
            line,
        }
    }

    /// The line so far, oldest first. Always at least two points.
    pub fn line(&self) -> &[QuotaSample] {
        &self.line
    }

    /// True when nothing was read between two neighboring points of the line.
    pub fn is_gap(first: QuotaSample, second: QuotaSample) -> bool {
        second.at.saturating_sub(first.at) > GAP
    }

    /// Project the average pace of the whole week so far, the steadiest estimate
    /// across quiet nights and busy days.
    pub fn outlook(&self) -> Outlook {
        let elapsed = self.latest.at.saturating_sub(self.start);
        if self.latest.used >= 100.0 {
            return Outlook::LimitReached;
        }
        if elapsed < MIN_PACE {
            return Outlook::TooEarly;
        }
        let pace = self.latest.used / elapsed as f64;
        let at_reset = pace * WEEK as f64;
        if at_reset < 100.0 {
            Outlook::AtReset(at_reset)
        } else {
            Outlook::RunsOut(self.start + (100.0 / pace) as u64)
        }
    }

    /// Where the projected line ends: at the reset, or where it reaches the limit.
    pub fn projection_end(&self) -> Option<QuotaSample> {
        match self.outlook() {
            Outlook::AtReset(used) => Some(QuotaSample { at: self.end, used }),
            Outlook::RunsOut(at) => Some(QuotaSample { at, used: 100.0 }),
            Outlook::TooEarly | Outlook::LimitReached => None,
        }
    }

    pub fn reading_at(&self, at: u64) -> Reading {
        if at > self.latest.at {
            return self
                .projection_end()
                .map_or(Reading::NotRecorded, |projection| {
                    Reading::Projected(between(self.latest, projection, at.min(projection.at)))
                });
        }
        let next = self
            .line
            .partition_point(|sample| sample.at < at)
            .clamp(1, self.line.len() - 1);
        let (first, second) = (self.line[next - 1], self.line[next]);
        if Self::is_gap(first, second) {
            Reading::NotRecorded
        } else {
            Reading::Recorded(between(first, second, at))
        }
    }
}

fn between(first: QuotaSample, second: QuotaSample, at: u64) -> f64 {
    let span = second.at.saturating_sub(first.at);
    if span == 0 {
        return second.used;
    }
    let along = at.saturating_sub(first.at) as f64 / span as f64;
    first.used + (second.used - first.used) * along
}

/// Save the weekly reading of each provider and return its week so far. A provider with no
/// running week has no reset time, or one that has passed, and is left out.
pub fn record(store: Option<&Store>, quotas: &[AccountQuota], now: u64) -> Vec<QuotaWeek> {
    quotas
        .iter()
        .filter_map(|quota| {
            let window = quota.windows.iter().find(|window| window.label == "7d")?;
            let end = window
                .resets_at
                .filter(|end| *end > now && *end <= now + WEEK)?;
            let latest = QuotaSample {
                at: now,
                used: 100.0 - window.remaining,
            };
            let samples = store
                .map(|store| {
                    let provider = quota.provider.command();
                    store.save_quota_sample(
                        provider,
                        now - now % BUCKET,
                        latest.used,
                        now - 2 * WEEK,
                    )?;
                    store.quota_samples(provider, end.saturating_sub(WEEK))
                })
                .transpose()
                .unwrap_or_else(|error| {
                    eprintln!("agtk: quota history unavailable: {error}");
                    None
                })
                .unwrap_or_default();
            Some(QuotaWeek::new(
                quota.provider,
                end,
                latest,
                samples
                    .into_iter()
                    .map(|(at, used)| QuotaSample { at, used }),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::quota::QuotaWindow;

    const END: u64 = 2_000_000_000;
    const START: u64 = END - WEEK;
    const HOUR: u64 = 60 * 60;
    const DAY: u64 = 24 * HOUR;

    fn week(elapsed: u64, used: f64, samples: &[(u64, f64)]) -> QuotaWeek {
        QuotaWeek::new(
            AgentProvider::Claude,
            END,
            QuotaSample {
                at: START + elapsed,
                used,
            },
            samples.iter().map(|(elapsed, used)| QuotaSample {
                at: START + elapsed,
                used: *used,
            }),
        )
    }

    fn quota(
        provider: AgentProvider,
        label: &str,
        remaining: f64,
        resets_at: Option<u64>,
    ) -> AccountQuota {
        AccountQuota {
            provider,
            windows: vec![QuotaWindow {
                label: label.to_owned(),
                remaining,
                resets_at,
            }],
            status: String::new(),
        }
    }

    #[test]
    fn pace_of_the_week_so_far_projects_to_the_reset() {
        let week = week(2 * DAY, 20.0, &[]);
        assert_eq!(week.outlook(), Outlook::AtReset(70.0));
        assert_eq!(
            week.projection_end(),
            Some(QuotaSample {
                at: END,
                used: 70.0
            })
        );
        assert_eq!(week.reading_at(START + 4 * DAY), Reading::Projected(40.0));
    }

    #[test]
    fn a_pace_over_the_limit_ends_the_projection_where_it_runs_out() {
        let week = week(2 * DAY, 50.0, &[]);
        assert_eq!(week.outlook(), Outlook::RunsOut(START + 4 * DAY));
        assert_eq!(
            week.projection_end(),
            Some(QuotaSample {
                at: START + 4 * DAY,
                used: 100.0
            })
        );
        assert_eq!(week.reading_at(START + 6 * DAY), Reading::Projected(100.0));
    }

    #[test]
    fn no_projection_early_in_the_week_or_at_the_limit() {
        assert_eq!(week(HOUR, 30.0, &[]).outlook(), Outlook::TooEarly);
        assert_eq!(week(HOUR, 30.0, &[]).projection_end(), None);
        assert_eq!(
            week(HOUR, 30.0, &[]).reading_at(START + DAY),
            Reading::NotRecorded
        );
        assert_eq!(week(HOUR, 100.0, &[]).outlook(), Outlook::LimitReached);
        assert_eq!(week(3 * DAY, 100.0, &[]).projection_end(), None);
    }

    #[test]
    fn an_unused_week_projects_nothing_used() {
        assert_eq!(week(DAY, 0.0, &[]).outlook(), Outlook::AtReset(0.0));
    }

    #[test]
    fn readings_only_cover_time_that_was_recorded() {
        let week = week(
            2 * DAY,
            30.0,
            &[
                (DAY, 10.0),
                (DAY + 10 * 60, 12.0),
                (2 * DAY - 10 * 60, 28.0),
            ],
        );
        // Before the first reading, while recording, and in the night nothing was read.
        assert_eq!(week.reading_at(START + HOUR), Reading::NotRecorded);
        assert_eq!(
            week.reading_at(START + DAY + 5 * 60),
            Reading::Recorded(11.0)
        );
        assert_eq!(
            week.reading_at(START + DAY + 6 * HOUR),
            Reading::NotRecorded
        );
        assert_eq!(week.reading_at(START + 2 * DAY), Reading::Recorded(30.0));
    }

    #[test]
    fn the_line_runs_from_the_start_of_the_week_to_the_latest_reading() {
        let week = week(DAY, 30.0, &[(DAY / 2, 10.0), (DAY, 29.0), (2 * DAY, 90.0)]);
        let line: Vec<_> = week
            .line()
            .iter()
            .map(|sample| (sample.at - START, sample.used))
            .collect();
        assert_eq!(line, [(0, 0.0), (DAY / 2, 10.0), (DAY, 30.0)]);
    }

    #[test]
    fn records_weekly_readings_and_returns_only_the_current_week() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("agtk.db")).unwrap();
        let now = START + 2 * DAY;
        let claude = quota(AgentProvider::Claude, "7d", 80.0, Some(END));
        store
            .save_quota_sample("claude", START - HOUR, 95.0, 0)
            .unwrap();
        record(Some(&store), std::slice::from_ref(&claude), now - DAY);

        let weeks = record(
            Some(&store),
            &[
                claude,
                quota(AgentProvider::Codex, "5h", 50.0, Some(now + HOUR)),
            ],
            now,
        );

        assert_eq!(weeks.len(), 1);
        assert_eq!(weeks[0].provider, AgentProvider::Claude);
        assert_eq!(weeks[0].latest.used, 20.0);
        assert_eq!(
            weeks[0].line().iter().map(|s| s.used).collect::<Vec<_>>(),
            [0.0, 20.0, 20.0, 20.0]
        );
        assert!(store.quota_samples("codex", 0).unwrap().is_empty());
    }

    #[test]
    fn old_readings_are_pruned() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("agtk.db")).unwrap();
        let now = START + DAY;
        store
            .save_quota_sample("claude", now - 3 * WEEK, 40.0, 0)
            .unwrap();
        record(
            Some(&store),
            &[quota(AgentProvider::Claude, "7d", 90.0, Some(END))],
            now,
        );
        assert_eq!(store.quota_samples("claude", 0).unwrap().len(), 1);
    }

    #[test]
    fn a_week_without_a_usable_reset_is_left_out() {
        let now = START + DAY;
        for resets_at in [None, Some(now), Some(now - 1), Some(now + 2 * WEEK)] {
            let quotas = [quota(AgentProvider::Claude, "7d", 90.0, resets_at)];
            assert!(record(None, &quotas, now).is_empty(), "{resets_at:?}");
        }
    }

    #[test]
    fn a_week_still_shows_without_a_store() {
        let now = START + DAY;
        let weeks = record(
            None,
            &[quota(AgentProvider::Claude, "7d", 90.0, Some(END))],
            now,
        );
        assert_eq!(weeks[0].line().len(), 2);
    }
}
