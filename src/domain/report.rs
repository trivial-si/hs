use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use chrono_tz::Tz;

use crate::domain::workday::Workday;
pub use crate::domain::workday::WorkdayRange as ReportRange;
use crate::error::{Error, Result};
use crate::model::{Entry, format_time};
use crate::time::Moment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Activity,
    Category,
    Tag,
    Day,
    Quadrant,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub total_minutes: i64,
    pub total_seconds: i64,
    pub groups: Vec<(String, i64)>,
    pub groups_seconds: Vec<(String, i64)>,
    pub unresolved_ids: Vec<i64>,
}

/// Workday containing an instant, in the moment's zone.
pub fn workday_at(now: &Moment, day_start_minutes: u32) -> Result<NaiveDate> {
    Workday::new(day_start_minutes)?.date_at(now)
}

fn legacy_now(now: NaiveDateTime) -> Moment {
    Moment {
        utc: now.and_utc(),
        zone: chrono_tz::UTC,
    }
}

// A timed entry uses the UTC axis. A legacy fact uses its original wall-clock axis.
struct ReportInterval {
    origin: NaiveDateTime,
    end: Option<NaiveDateTime>,
    zone: Option<Tz>,
}

impl ReportInterval {
    fn new(entry: &Entry, now_utc: DateTime<Utc>, now_local: NaiveDateTime) -> Self {
        let (origin, end, zone) = if let Some(clock) = &entry.clock {
            let end = clock.end_utc.or_else(|| entry.running.then_some(now_utc));
            (
                clock.start_utc.naive_utc(),
                end.map(|t| t.naive_utc()),
                Some(clock.zone),
            )
        } else {
            (
                entry.start,
                entry.end.or_else(|| entry.running.then_some(now_local)),
                None,
            )
        };
        Self { origin, end, zone }
    }

    fn in_range(&self, bounds: &ReportRange) -> bool {
        if bounds.start >= bounds.end || self.origin >= bounds.end {
            return false;
        }
        // Include zero-length entries and open imports with unknown duration.
        self.origin >= bounds.start || self.end.is_none_or(|end| end > bounds.start)
    }

    fn checked_end(&self, id: i64) -> Result<Option<NaiveDateTime>> {
        if self.end.is_some_and(|end| end < self.origin) {
            return Err(Error::Input(format!(
                "entry {id} ends before it starts; check the clock or time record"
            )));
        }
        Ok(self.end)
    }

    fn clipped(
        &self,
        id: i64,
        bounds: &ReportRange,
    ) -> Result<Option<(NaiveDateTime, NaiveDateTime)>> {
        Ok(self
            .checked_end(id)?
            .map(|end| (self.origin.max(bounds.start), end.min(bounds.end))))
    }

    fn seconds(&self, from: NaiveDateTime, to: NaiveDateTime) -> i64 {
        // Count complete units since the entry's original start. Adjacent clips
        // then sum to the original duration even for fractional-second imports.
        let from = from - self.origin;
        let to = to - self.origin;
        if self.zone.is_some() {
            to.num_seconds() - from.num_seconds()
        } else {
            (to.num_minutes() - from.num_minutes()) * 60
        }
    }

    fn group_days(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
        workday: Workday,
        groups: &mut BTreeMap<Option<String>, i64>,
    ) -> Result<()> {
        if start >= end {
            return Ok(());
        }
        let mut day = workday.date_on_axis(start, self.zone)?;
        let mut cursor = start;
        while cursor < end {
            let next_day = day
                .succ_opt()
                .ok_or_else(|| Error::Input("workday out of range".into()))?;
            let next = workday.start_on_axis(next_day, self.zone).min(end);
            if next > cursor {
                *groups.entry(Some(day.to_string())).or_default() += self.seconds(cursor, next);
                cursor = next;
            }
            day = next_day;
        }
        Ok(())
    }
}

pub fn in_range(entry: &Entry, range: &ReportRange, now: NaiveDateTime) -> bool {
    let interval = ReportInterval::new(entry, now.and_utc(), now);
    interval.in_range(&range.on_axis(interval.zone))
}

pub fn in_range_at(entry: &Entry, range: &ReportRange, now: &Moment) -> Result<bool> {
    range.validate()?;
    let interval = ReportInterval::new(entry, now.utc, now.local()?);
    Ok(interval.in_range(&range.on_axis(interval.zone)))
}

/// Full entry duration. Unfinished imports have an unknown duration.
pub fn entry_seconds_at(entry: &Entry, now: &Moment) -> Result<Option<i64>> {
    let interval = ReportInterval::new(entry, now.utc, now.local()?);
    Ok(interval
        .checked_end(entry.id)?
        .map(|end| interval.seconds(interval.origin, end)))
}

pub fn summarize(
    entries: &[Entry],
    range: &ReportRange,
    group: GroupBy,
    now: NaiveDateTime,
) -> Result<Report> {
    summarize_at(entries, range, group, &legacy_now(now))
}

fn checked_total(total: i64, duration: i64) -> Result<i64> {
    total
        .checked_add(duration)
        .ok_or_else(|| Error::Input("Report total exceeds the supported number of seconds".into()))
}

pub fn summarize_at(
    entries: &[Entry],
    range: &ReportRange,
    group: GroupBy,
    now: &Moment,
) -> Result<Report> {
    range.validate()?;
    let mut groups = BTreeMap::<Option<String>, i64>::new();
    let mut unresolved_ids = Vec::new();
    let mut total_seconds = 0_i64;
    let now_local = now.local()?;
    for entry in entries {
        let interval = ReportInterval::new(entry, now.utc, now_local);
        let bounds = range.on_axis(interval.zone);
        if !interval.in_range(&bounds) {
            continue;
        }
        let Some((start, end)) = interval.clipped(entry.id, &bounds)? else {
            unresolved_ids.push(entry.id);
            continue;
        };
        let duration = interval.seconds(start, end);
        // Each nonnegative interval contributes at most once to each group.
        // Checking the grand total also bounds every group accumulator below.
        total_seconds = checked_total(total_seconds, duration)?;
        match group {
            GroupBy::Activity => {
                *groups.entry(Some(entry.activity.clone())).or_default() += duration
            }
            GroupBy::Category => *groups.entry(entry.category.clone()).or_default() += duration,
            GroupBy::Quadrant => {
                let quadrant = entry.task.and_then(|task| task.quadrant);
                *groups
                    .entry(quadrant.map(|q| q.label().to_owned()))
                    .or_default() += duration;
            }
            GroupBy::Tag => {
                if entry.tags.is_empty() {
                    *groups.entry(None).or_default() += duration;
                }
                for tag in entry.tags.iter().collect::<BTreeSet<_>>() {
                    *groups.entry(Some(tag.clone())).or_default() += duration;
                }
            }
            GroupBy::Day => interval.group_days(start, end, range.workday(), &mut groups)?,
        }
    }
    unresolved_ids.sort_unstable();
    let missing_label = match group {
        GroupBy::Tag => "(untagged)",
        GroupBy::Quadrant => "Unclassified",
        _ => "(uncategorized)",
    };
    // In a collision, quote literal names so a missing value, a user-supplied
    // label, and even a label that itself contains quotes stay distinguishable.
    let quote_names =
        groups.contains_key(&None) && groups.contains_key(&Some(missing_label.to_owned()));
    let mut display_groups: Vec<_> = groups
        .into_iter()
        .map(|(key, seconds)| {
            let label = match key {
                Some(name) if quote_names => format!("{name:?}"),
                Some(name) => name,
                None => missing_label.to_owned(),
            };
            (label, seconds)
        })
        .collect();
    display_groups.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(Report {
        total_minutes: total_seconds / 60,
        total_seconds,
        groups: display_groups
            .iter()
            .map(|(label, sec)| (label.clone(), sec / 60))
            .collect(),
        groups_seconds: display_groups,
        unresolved_ids,
    })
}

fn tsv_field(field: &str) -> String {
    if field.contains(['\t', '\r', '\n', '"']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// Keep original timestamps. The minutes column describes only the selected
/// part of the interval. A missing duration or end time is an empty field.
pub fn export_tsv(entries: &[Entry], range: &ReportRange, now: NaiveDateTime) -> Result<String> {
    export_tsv_at(entries, range, &legacy_now(now))
}

fn utc_text(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

pub fn export_tsv_at(entries: &[Entry], range: &ReportRange, now: &Moment) -> Result<String> {
    range.validate()?;
    let mut text = String::from(
        "id\tstart\tend\tactivity\tcategory\tdescription\ttags\tminutes\tstate\tseconds\tstart_utc\tend_utc\ttimezone\ttask_id\tquadrant\n",
    );
    let now_local = now.local()?;
    let mut rows: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let interval = ReportInterval::new(entry, now.utc, now_local);
            let bounds = range.on_axis(interval.zone);
            interval
                .in_range(&bounds)
                .then_some((entry, interval, bounds))
        })
        .collect();
    rows.sort_by_key(|(entry, _, _)| (entry.start, entry.id));
    for (entry, interval, bounds) in rows {
        let seconds = interval
            .clipped(entry.id, &bounds)?
            .map(|(start, end)| interval.seconds(start, end));
        let state = match (entry.end, entry.running) {
            (Some(_), _) => "completed",
            (None, true) => "running",
            (None, false) => "unresolved",
        };
        let fields = [
            entry.id.to_string(),
            format_time(entry.start),
            entry.end.map(format_time).unwrap_or_default(),
            entry.activity.clone(),
            entry.category.clone().unwrap_or_default(),
            entry.description.clone(),
            serde_json::to_string(&entry.tags)?,
            seconds.map(|s| (s / 60).to_string()).unwrap_or_default(),
            state.to_owned(),
            seconds.map(|s| s.to_string()).unwrap_or_default(),
            entry
                .clock
                .as_ref()
                .map(|c| utc_text(c.start_utc))
                .unwrap_or_default(),
            entry
                .clock
                .as_ref()
                .and_then(|c| c.end_utc)
                .map(utc_text)
                .unwrap_or_default(),
            entry
                .clock
                .as_ref()
                .map(|c| c.zone.name())
                .unwrap_or_default()
                .to_owned(),
            entry
                .task
                .map(|task| task.id.to_string())
                .unwrap_or_default(),
            entry
                .task
                .and_then(|task| task.quadrant)
                .map(|q| q.key())
                .unwrap_or_default()
                .to_owned(),
        ];
        let row: Vec<_> = fields.iter().map(|field| tsv_field(field)).collect();
        text.push_str(&row.join("\t"));
        text.push('\n');
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn time(day: u32, hour: u32, minute: u32, second: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, day)
            .unwrap()
            .and_hms_opt(hour, minute, second)
            .unwrap()
    }

    fn entry(start: NaiveDateTime, end: Option<NaiveDateTime>) -> Entry {
        Entry {
            id: 1,
            clock: None,
            task: None,
            start,
            end,
            running: false,
            activity: "Code".into(),
            category: None,
            description: String::new(),
            tags: Vec::new(),
        }
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().to_utc()
    }

    fn clocked(start: DateTime<Utc>, end: Option<DateTime<Utc>>, zone: Tz) -> Entry {
        let local = |instant: DateTime<Utc>| instant.with_timezone(&zone).naive_local();
        let mut row = entry(local(start), end.map(local));
        row.running = end.is_none();
        row.clock = Some(crate::model::EntryClock {
            start_utc: start,
            end_utc: end,
            zone,
        });
        row
    }

    #[test]
    fn short_focus_segments_sum_before_minute_conversion() {
        let start = utc("2026-11-16T09:00:00Z");
        let now = Moment {
            utc: start + Duration::seconds(80),
            zone: chrono_tz::Asia::Tokyo,
        };
        let entries = [0, 30, 60].map(|s| {
            let from = start + Duration::seconds(s);
            let end = (s < 60).then_some(from + Duration::seconds(20));
            clocked(from, end, chrono_tz::Europe::Zagreb)
        });
        let range = ReportRange::new(start.date_naive(), start.date_naive(), 0).unwrap();
        let result = summarize_at(&entries, &range, GroupBy::Activity, &now).unwrap();
        assert_eq!(result.total_seconds, 60);
        assert_eq!(result.total_minutes, 1);
        assert_eq!(result.groups_seconds, [("Code".to_string(), 60)]);
        assert_eq!(result.groups, [("Code".to_string(), 1)]);
        let tsv = export_tsv_at(&entries, &range, &now).unwrap();
        assert!(tsv.contains("\t0\trunning\t20\t2026-11-16T09:01:00Z\t\tEurope/Zagreb"));
    }

    #[test]
    fn fold_preserves_positive_duration_and_uses_first_workday_boundary() {
        let row = clocked(
            utc("2026-10-25T00:20:00Z"),
            Some(utc("2026-10-25T01:10:00Z")),
            chrono_tz::Europe::Zagreb,
        );
        assert!(row.end.unwrap() < row.start);
        let now = Moment {
            utc: utc("2026-10-25T05:00:00Z"),
            zone: chrono_tz::America::New_York,
        };
        let sunday = row.start.date();
        let week = ReportRange::new(sunday - Duration::days(6), sunday, 150).unwrap();
        let entries = [row];
        let report = summarize_at(&entries, &week, GroupBy::Day, &now).unwrap();
        assert_eq!(report.total_seconds, 50 * 60);
        assert_eq!(
            report.groups_seconds,
            [("2026-10-24".into(), 600), ("2026-10-25".into(), 2400)]
        );
        let sunday_range = ReportRange::new(sunday, sunday, 150).unwrap();
        assert!(in_range_at(&entries[0], &sunday_range, &now).unwrap());
        assert_eq!(
            summarize_at(&entries, &sunday_range, GroupBy::Day, &now)
                .unwrap()
                .total_seconds,
            2400
        );
        // From the *second* occurrence of 02:05, the first 02:30 boundary has already passed.
        let later_fold = clocked(
            utc("2026-10-25T01:05:00Z"),
            Some(utc("2026-10-25T01:10:00Z")),
            chrono_tz::Europe::Zagreb,
        );
        assert_eq!(
            summarize_at(&[later_fold], &sunday_range, GroupBy::Day, &now)
                .unwrap()
                .groups_seconds,
            [("2026-10-25".into(), 300)]
        );
    }

    #[test]
    fn missing_workday_boundary_begins_at_first_valid_instant() {
        let row = clocked(
            utc("2026-03-29T00:55:00Z"),
            Some(utc("2026-03-29T01:05:00Z")),
            chrono_tz::Europe::Zagreb,
        );
        let sunday = row.start.date();
        let week = ReportRange::new(sunday - Duration::days(6), sunday, 150).unwrap();
        let now = Moment {
            utc: utc("2026-03-29T04:30:00Z"),
            zone: chrono_tz::UTC,
        };
        let report = summarize_at(&[row], &week, GroupBy::Day, &now).unwrap();
        assert_eq!(report.total_seconds, 600);
        assert_eq!(
            report.groups_seconds,
            [("2026-03-28".into(), 300), ("2026-03-29".into(), 300)]
        );
    }

    #[test]
    fn report_rejects_overflow_and_unrepresentable_invocation_dates() {
        assert_eq!(checked_total(i64::MAX - 60, 60).unwrap(), i64::MAX);
        assert!(checked_total(i64::MAX - 59, 60).is_err());
        let outside = Moment::at(DateTime::<Utc>::MAX_UTC, chrono_tz::Asia::Tokyo);
        let date = time(16, 0, 0, 0).date();
        let range = ReportRange::new(date, date, 0).unwrap();
        let mut row = entry(time(16, 1, 0, 0), None);
        row.running = true;
        assert!(workday_at(&outside, 0).is_err());
        assert!(in_range_at(&row, &range, &outside).is_err());
        assert!(entry_seconds_at(&row, &outside).is_err());
        assert!(summarize_at(std::slice::from_ref(&row), &range, GroupBy::Day, &outside).is_err());
        assert!(export_tsv_at(&[row], &range, &outside).is_err());
    }

    #[test]
    fn manually_constructed_invalid_range_fails_before_emitting_an_empty_report() {
        let start = time(16, 1, 0, 0);
        let row = entry(start, Some(start));
        let now = legacy_now(start);
        let inverted = ReportRange {
            start,
            end: start - Duration::minutes(1),
        };
        assert!(in_range_at(&row, &inverted, &now).is_err());
        assert!(summarize_at(&[], &inverted, GroupBy::Day, &now).is_err());
        assert!(export_tsv_at(&[], &inverted, &now).is_err());
        let empty = ReportRange { start, end: start };
        assert!(!in_range_at(&row, &empty, &now).unwrap());
        assert_eq!(
            summarize_at(&[row], &empty, GroupBy::Day, &now)
                .unwrap()
                .total_seconds,
            0
        );
    }

    #[test]
    fn current_workday_across_repeated_boundary() {
        let zone = chrono_tz::Europe::Zagreb;
        let before = Moment::at(utc("2026-10-25T00:05:00Z"), zone);
        let after = Moment::at(utc("2026-10-25T01:05:00Z"), zone);
        assert_eq!(before.local().unwrap(), after.local().unwrap());
        assert_eq!(workday_at(&before, 150).unwrap().to_string(), "2026-10-24");
        assert_eq!(workday_at(&after, 150).unwrap().to_string(), "2026-10-25");
        assert!(workday_at(&after, 1440).is_err());
    }

    #[test]
    fn full_entry_seconds_keep_precision_per_clock_type() {
        let start = utc("2026-10-25T00:59:55Z");
        let now = Moment::at(utc("2026-10-25T01:00:15Z"), chrono_tz::Europe::Zagreb);
        assert_eq!(
            entry_seconds_at(&clocked(start, None, now.zone), &now).unwrap(),
            Some(20)
        );
        let legacy = entry(start.naive_utc(), Some(now.utc.naive_utc()));
        assert_eq!(entry_seconds_at(&legacy, &now).unwrap(), Some(0));
        assert_eq!(
            entry_seconds_at(&entry(start.naive_utc(), None), &now).unwrap(),
            None
        );
    }

    #[test]
    fn seven_days_sum_to_week_across_workday_boundaries() {
        let monday = time(16, 0, 0, 0).date();
        let sunday = monday + Duration::days(6);
        let now = time(25, 12, 0, 0);
        let first = entry(time(15, 23, 0, 0), Some(time(17, 7, 0, 0)));
        let second = entry(time(18, 5, 29, 30), Some(time(18, 5, 30, 30)));
        let third = entry(time(22, 23, 0, 0), Some(time(23, 7, 0, 0)));
        let entries = [first, second, third];
        let week = ReportRange::new(monday, sunday, 330).unwrap();
        let report = summarize(&entries, &week, GroupBy::Day, now).unwrap();
        let daily: Vec<_> = (0..7)
            .map(|offset| {
                let day = monday + Duration::days(offset);
                let range = ReportRange::new(day, day, 330).unwrap();
                summarize(&entries, &range, GroupBy::Activity, now)
                    .unwrap()
                    .total_minutes
            })
            .collect();
        assert_eq!(daily, vec![1440, 90, 1, 0, 0, 0, 390]);
        assert_eq!(daily.iter().sum::<i64>(), report.total_minutes);
        assert_eq!(
            report
                .groups
                .iter()
                .map(|(_, minutes)| minutes)
                .sum::<i64>(),
            report.total_minutes
        );
    }

    #[test]
    fn tag_totals_overlap_and_unresolved_end_is_unknown() {
        let start = time(16, 8, 0, 0);
        let now = start + Duration::minutes(10);
        let range = ReportRange::new(start.date(), start.date(), 0).unwrap();
        let mut running = entry(start, None);
        running.running = true;
        running.tags = vec!["a".into(), "b".into(), "a".into()];
        let mut old = entry(start - Duration::days(2), None);
        old.id = 2;
        let entries = [running, old];
        let report = summarize(&entries, &range, GroupBy::Tag, now).unwrap();
        assert_eq!(report.total_minutes, 10);
        assert_eq!(report.groups, vec![("a".into(), 10), ("b".into(), 10)]);
        assert_eq!(report.unresolved_ids, vec![2]);
        assert!(
            export_tsv(&entries, &range, now)
                .unwrap()
                .contains("\t\tunresolved\t\t\t\t\t\t\n")
        );
        assert!(summarize(&entries, &range, GroupBy::Tag, start - Duration::minutes(1)).is_err());
    }

    #[test]
    fn export_preserves_text_and_clips_duration() {
        let mut row = entry(time(15, 23, 45, 0), Some(time(16, 0, 15, 0)));
        row.activity = "Café\t工".into();
        row.description = "first\n\"second\"".into();
        row.tags.push("🌻".into());
        let day = time(16, 0, 0, 0);
        let range = ReportRange::new(day.date(), day.date(), 0).unwrap();
        let tsv = export_tsv(&[row], &range, day).unwrap();
        assert_eq!(
            tsv.lines().next().unwrap(),
            "id\tstart\tend\tactivity\tcategory\tdescription\ttags\tminutes\tstate\tseconds\tstart_utc\tend_utc\ttimezone\ttask_id\tquadrant"
        );
        assert!(tsv.contains("2026-11-15 23:45:00\t2026-11-16 00:15:00"));
        assert!(tsv.contains("\"Café\t工\"\t\t\"first\n\"\"second\"\"\""));
        assert!(tsv.contains("\"[\"\"🌻\"\"]\"\t15\tcompleted\t900\t\t\t\t\t\n"));
    }

    #[test]
    fn quadrant_totals_use_each_entry_snapshot_and_include_unclassified_work() {
        use crate::model::EntryTask;
        use crate::tasks::Quadrant;

        let mut planned = entry(time(15, 23, 50, 0), Some(time(16, 0, 10, 0)));
        planned.task = Some(EntryTask {
            id: 7,
            quadrant: Some(Quadrant::Plan),
        });
        let mut urgent = clocked(
            utc("2026-11-16T08:00:00Z"),
            Some(utc("2026-11-16T08:00:20Z")),
            chrono_tz::UTC,
        );
        urgent.task = Some(EntryTask {
            id: 7,
            quadrant: Some(Quadrant::Do),
        });
        let mut untriaged = entry(time(16, 9, 0, 0), Some(time(16, 9, 3, 0)));
        untriaged.task = Some(EntryTask {
            id: 8,
            quadrant: None,
        });
        let unlinked = entry(time(16, 10, 0, 0), Some(time(16, 10, 2, 0)));
        let entries = [planned, urgent, untriaged, unlinked];
        let now = time(16, 12, 0, 0);
        let range = ReportRange::new(now.date(), now.date(), 0).unwrap();
        let report = summarize(&entries, &range, GroupBy::Quadrant, now).unwrap();
        assert_eq!(
            report.groups_seconds,
            [
                ("Do".into(), 20),
                ("Plan".into(), 600),
                ("Unclassified".into(), 300)
            ]
        );
        assert_eq!(report.total_seconds, 920);
        let tsv = export_tsv(&entries, &range, now).unwrap();
        let fields: Vec<_> = tsv
            .lines()
            .map(|line| line.split('\t').skip(13).collect::<Vec<_>>())
            .collect();
        assert_eq!(
            fields,
            [
                ["task_id", "quadrant"],
                ["7", "plan"],
                ["7", "do"],
                ["8", ""],
                ["", ""]
            ]
        );
    }

    #[test]
    fn named_groups_are_distinct_from_missing_category_or_tags() {
        let start = time(16, 8, 0, 0);
        let range = ReportRange::new(start.date(), start.date(), 0).unwrap();
        let missing = entry(start, Some(start + Duration::minutes(15)));
        let mut named = entry(start, Some(start + Duration::minutes(60)));
        named.category = Some("(uncategorized)".into());
        named.tags = vec!["(untagged)".into()];
        let mut quoted = entry(start, Some(start + Duration::minutes(5)));
        quoted.category = Some("\"(uncategorized)\"".into());
        quoted.tags = vec!["\"(untagged)\"".into()];
        let now = time(16, 12, 0, 0);
        let records = [missing, named, quoted];
        for (group, label) in [
            (GroupBy::Category, "(uncategorized)"),
            (GroupBy::Tag, "(untagged)"),
        ] {
            let report = summarize(&records, &range, group, now).unwrap();
            let groups: BTreeMap<_, _> = report.groups.into_iter().collect();
            assert_eq!(groups.len(), 3);
            assert_eq!(groups[label], 15);
            assert_eq!(groups[&format!("{label:?}")], 60);
            assert_eq!(groups[&format!("{:?}", format!("\"{label}\""))], 5);
            assert_eq!(report.total_minutes, 80);

            let ordinary = summarize(&records[..1], &range, group, now).unwrap();
            assert_eq!(ordinary.groups, vec![(label.to_owned(), 15)]);
        }
    }
}
