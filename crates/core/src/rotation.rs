//! Rotation recency: the raw material for the anti-curry engine. For every
//! (axis, value) tag pair seen in the log — cuisine=sichuan, protein=pork,
//! format=braise — how recently and how often has it been cooked? Rotation is
//! about the person, not the kitchen: curry at the cottage still counts.

use std::collections::BTreeMap;

use jiff::civil::Date;

use crate::types::LogEntry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recency {
    /// Most recent cook date for this (axis, value), no later than `today`.
    pub last_made: Date,
    /// Cooks within the trailing window (inclusive of today).
    pub in_window: u32,
}

impl Recency {
    pub fn days_since(&self, today: Date) -> i32 {
        (today - self.last_made).get_days()
    }
}

/// Recency per (axis, value) over the log as of `today`. Entries dated after
/// `today` are ignored — time is an input, and the future hasn't been cooked
/// yet. All cook kinds count: a bake rotates the "format" axis like any meal.
pub fn recency(
    log: &[LogEntry],
    today: Date,
    window_days: u16,
) -> BTreeMap<(String, String), Recency> {
    let mut out: BTreeMap<(String, String), Recency> = BTreeMap::new();
    for entry in log {
        if entry.date > today {
            continue;
        }
        let in_window = u32::from((today - entry.date).get_days() < i32::from(window_days));
        for (axis, value) in &entry.tags {
            out.entry((axis.clone(), value.clone()))
                .and_modify(|r| {
                    r.last_made = r.last_made.max(entry.date);
                    r.in_window += in_window;
                })
                .or_insert(Recency {
                    last_made: entry.date,
                    in_window,
                });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::types::CookKind;

    fn entry(date: Date, tags: &[(&str, &str)]) -> LogEntry {
        LogEntry {
            date,
            kind: CookKind::Meal,
            recipe: None,
            title: "dish".into(),
            location: "home".into(),
            servings: 2,
            verdict: "fine".into(),
            tags: tags
                .iter()
                .map(|(a, v)| (a.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn tracks_last_made_and_window_counts_per_axis_value() {
        let log = vec![
            entry(Date::constant(2026, 7, 1), &[("cuisine", "indian"), ("format", "curry")]),
            entry(Date::constant(2026, 7, 20), &[("cuisine", "sichuan"), ("format", "stir-fry")]),
            entry(Date::constant(2026, 7, 27), &[("cuisine", "indian"), ("format", "curry")]),
        ];
        let today = Date::constant(2026, 7, 29);
        let r = recency(&log, today, 14);

        let curry = &r[&("format".to_string(), "curry".to_string())];
        assert_eq!(curry.last_made, Date::constant(2026, 7, 27));
        assert_eq!(curry.in_window, 1); // July 1 is outside the 14-day window
        assert_eq!(curry.days_since(today), 2);

        let stir_fry = &r[&("format".to_string(), "stir-fry".to_string())];
        assert_eq!(stir_fry.in_window, 1);
    }

    #[test]
    fn future_entries_are_ignored() {
        let log = vec![entry(Date::constant(2026, 8, 1), &[("cuisine", "thai")])];
        let r = recency(&log, Date::constant(2026, 7, 29), 14);
        assert!(r.is_empty());
    }

    #[test]
    fn untagged_entries_contribute_nothing() {
        let log = vec![LogEntry {
            tags: BTreeMap::new(),
            ..entry(Date::constant(2026, 7, 28), &[])
        }];
        let r = recency(&log, Date::constant(2026, 7, 29), 14);
        assert!(r.is_empty());
    }
}
