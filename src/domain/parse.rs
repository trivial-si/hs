use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

use crate::domain::workday::Workday;
use crate::error::{Error, Result};
use crate::model::{ActivityTemplate, NewEntry, minute};

fn invalid(message: impl Into<String>) -> Error {
    Error::Input(message.into())
}

fn add_minutes(time: NaiveDateTime, minutes: i64) -> Result<NaiveDateTime> {
    TimeDelta::try_minutes(minutes)
        .and_then(|delta| time.checked_add_signed(delta))
        .ok_or_else(|| invalid("time is outside the supported range"))
}

#[derive(Clone, Copy)]
enum TimeSpec {
    Clock(NaiveTime),
    Dated(NaiveDateTime),
    Offset(i64),
}

impl TimeSpec {
    fn resolve(self, reference: NaiveDateTime, workday: Workday) -> Result<NaiveDateTime> {
        match self {
            Self::Dated(timestamp) => Ok(timestamp),
            Self::Offset(minutes) => add_minutes(minute(reference), minutes),
            Self::Clock(clock) => workday.civil_clock(clock, reference),
        }
    }
}

fn digits(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_digit).count()
}

fn clock_fields(text: &str) -> Option<(&str, &str, &str)> {
    let (leading, rest) = text.split_at(digits(text));
    if let Some(after_separator) = rest.strip_prefix([':', '.', ',']) {
        if !(1..=2).contains(&leading.len()) || digits(after_separator) != 2 {
            return None;
        }
        let (minutes, tail) = after_separator.split_at(2);
        Some((leading, minutes, tail))
    } else if leading.len() == 4 {
        let (hours, minutes) = leading.split_at(2);
        Some((hours, minutes, rest))
    } else {
        None
    }
}

fn time_boundary(text: &str) -> bool {
    text.is_empty() || text.starts_with(char::is_whitespace) || text.starts_with('-')
}

fn clock_prefix(text: &str) -> Result<(NaiveTime, &str)> {
    let (hours, minutes, rest) =
        clock_fields(text).ok_or_else(|| invalid("expected a time such as 09:30"))?;
    // clock_fields verifies that both fields consist of at most two ASCII digits.
    let clock = NaiveTime::from_hms_opt(hours.parse().unwrap(), minutes.parse().unwrap(), 0)
        .ok_or_else(|| invalid("clock time must be between 00:00 and 23:59"))?;
    Ok((clock, rest))
}

fn starts_date(text: &str) -> bool {
    // Distinguish YYYY-MM-DD from a pair of compact clocks such as 2019-2025.
    let Some(after_year) = text.get(4..).and_then(|tail| tail.strip_prefix('-')) else {
        return false;
    };
    let month_digits = digits(after_year);
    (1..=2).contains(&month_digits) && after_year[month_digits..].starts_with('-')
}

// Leave separators in the remainder so a '-' joining two times can be distinguished
// from the sign of a relative time after whitespace.
fn time_prefix(text: &str) -> Result<Option<(TimeSpec, &str)>> {
    let leading_digits = digits(text);
    let (spec, rest) = if leading_digits == 4 && starts_date(text) {
        let date_text = text
            .get(..10)
            .ok_or_else(|| invalid("expected YYYY-MM-DD HH:MM"))?;
        let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d")
            .map_err(|_| invalid("invalid calendar date; expected YYYY-MM-DD HH:MM"))?;
        let tail = &text[10..];
        let time_text = if let Some(after_t) = tail.strip_prefix('T') {
            after_t
        } else if tail.starts_with(char::is_whitespace) {
            tail.trim_start()
        } else {
            return Err(invalid("expected a clock time after the date"));
        };
        let (clock, rest) = clock_prefix(time_text)?;
        (TimeSpec::Dated(date.and_time(clock)), rest)
    } else if (leading_digits > 0 && text[leading_digits..].starts_with(':'))
        || clock_fields(text).is_some_and(|(_, _, rest)| time_boundary(rest))
    {
        let (clock, rest) = clock_prefix(text)?;
        (TimeSpec::Clock(clock), rest)
    } else if text.starts_with(['-', '+']) && digits(&text[1..]) > 0 {
        let end = 1 + digits(&text[1..]);
        let minutes = text[..end]
            .parse()
            .map_err(|_| invalid("invalid minute offset"))?;
        (TimeSpec::Offset(minutes), &text[end..])
    } else {
        return Ok(None);
    };
    if !time_boundary(rest) {
        return Err(invalid("unexpected text after the time"));
    }
    Ok(Some((spec, rest)))
}

fn exact_time(input: &str) -> Result<TimeSpec> {
    let input = input.trim();
    if input.eq_ignore_ascii_case("now") {
        return Ok(TimeSpec::Offset(0));
    }
    let Some((spec, "")) = time_prefix(input)? else {
        return Err(invalid(
            "expected a clock time, dated time or signed minute offset",
        ));
    };
    Ok(spec)
}

/// Recognize a bare time of day without assigning it a calendar date.
/// Other valid timestamp forms must be resolved using their own references.
pub fn standalone_clock(input: &str) -> Result<Option<NaiveTime>> {
    Ok(match exact_time(input)? {
        TimeSpec::Clock(clock) => Some(clock),
        _ => None,
    })
}

/// Resolve one local timestamp, time of day, or signed minute offset.
pub fn parse_time(
    input: &str,
    reference: NaiveDateTime,
    day_start_minutes: u32,
) -> Result<NaiveDateTime> {
    let workday = Workday::new(day_start_minutes)?;
    exact_time(input)?.resolve(reference, workday)
}

fn resolve_end(
    spec: TimeSpec,
    start: NaiveDateTime,
    reference: NaiveDateTime,
    workday: Workday,
) -> Result<NaiveDateTime> {
    let base = if matches!(spec, TimeSpec::Offset(_)) {
        reference
    } else {
        start
    };
    let mut end = spec.resolve(base, workday)?;
    if matches!(spec, TimeSpec::Clock(_)) && end < minute(start) {
        end = add_minutes(end, 24 * 60)?;
    }
    if end < start {
        return Err(invalid("end time precedes start time"));
    }
    Ok(end)
}

/// Resolve an end relative to the entry start. A bare clock can roll into
/// the next workday. A signed offset is measured from the supplied start.
pub fn parse_end_time(
    input: &str,
    start: NaiveDateTime,
    day_start_minutes: u32,
) -> Result<NaiveDateTime> {
    let workday = Workday::new(day_start_minutes)?;
    resolve_end(exact_time(input)?, start, start, workday)
}

fn parse_end(
    tail: &str,
    start: NaiveDateTime,
    now: NaiveDateTime,
    workday: Workday,
) -> Result<(Option<NaiveDateTime>, &str)> {
    let trimmed = tail.trim_start();
    // A standalone dash (or one adjacent to the start, or followed by a clock)
    // is a range separator. A signed offset after a space is relative to now.
    let after_separator = trimmed.strip_prefix('-').filter(|after| {
        tail.starts_with('-')
            || after.is_empty()
            || after.starts_with(char::is_whitespace)
            || clock_fields(after).is_some_and(|(_, _, rest)| time_boundary(rest))
    });
    let text = after_separator.unwrap_or(trimmed).trim_start();
    let (end, rest) = if let Some((spec, rest)) = time_prefix(text)? {
        (resolve_end(spec, start, now, workday)?, rest)
    } else {
        let length = digits(text);
        if length == 0
            || !text[length..].is_empty() && !text[length..].starts_with(char::is_whitespace)
        {
            if after_separator.is_some() {
                return Err(invalid("expected an end time or a duration in minutes"));
            }
            return Ok((None, trimmed));
        }
        let minutes = text[..length]
            .parse()
            .map_err(|_| invalid("invalid duration in minutes"))?;
        (add_minutes(start, minutes)?, &text[length..])
    };
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return Err(invalid("expected the activity after the end time"));
    }
    Ok((Some(end), rest.trim_start()))
}

fn add_tag(tags: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if !name.is_empty() && !tags.iter().any(|tag| tag == name) {
        tags.push(name.to_owned());
    }
}

fn tag_start(text: &str) -> bool {
    text.strip_prefix('#')
        .and_then(|after| after.chars().next())
        .is_some_and(char::is_alphabetic)
}

// A comma ends the activity field even when using shorthand tags in its head.
// Parse that head separately so a note after it cannot silently become a tag.
fn head_tags(head: &str) -> (&str, &str) {
    let start = head.char_indices().find_map(|(i, c)| {
        (c == '#' && tag_start(&head[i..]) && head[..i].ends_with(char::is_whitespace)).then_some(i)
    });
    head.split_at(start.unwrap_or(head.len()))
}

fn note_tags(note: &str) -> (&str, &str) {
    // Only the final run of comma-separated tag fields is a tag list.
    // Earlier hashtags can be part of a sentence followed by more notes.
    let mut tail = note.len();
    let mut field_end = tail;
    for field in note.rsplit(',') {
        if !tag_start(field.trim_start()) {
            break;
        }
        tail = field_end - field.len();
        field_end = tail.saturating_sub(1); // step over the preceding comma
    }
    if tail == note.len() {
        return (note, "");
    }
    let (description, tags) = note.split_at(tail);
    (description.trim_end().trim_end_matches(','), tags)
}

/// Parse quick entry text, keeping the invocation precision for an implicit start.
pub fn parse_live_entry(
    input: &str,
    now: NaiveDateTime,
    day_start_minutes: u32,
) -> Result<NewEntry> {
    let workday = Workday::new(day_start_minutes)?;
    let input = input.trim();
    let (start, end, text) = if let Some((spec, tail)) = time_prefix(input)? {
        let start = spec.resolve(now, workday)?;
        let (end, text) = parse_end(tail, start, now, workday)?;
        (start, end, text)
    } else {
        (now, None, input)
    };
    let mut entry = activity_details(text)?.starting_at(start);
    entry.end = end;
    Ok(entry)
}

/// Parse a Hamster quick entry using minute precision for its implicit start.
pub fn parse_entry(input: &str, now: NaiveDateTime, day_start_minutes: u32) -> Result<NewEntry> {
    parse_live_entry(input, minute(now), day_start_minutes)
}

/// Parse reusable activity details. An explicit time belongs to a tracked interval.
pub fn parse_activity_template(input: &str) -> Result<ActivityTemplate> {
    let input = input.trim();
    if time_prefix(input)?.is_some() {
        return Err(invalid(
            "Omit clock times, minute offsets and intervals from the activity template.",
        ));
    }
    activity_details(input)
}

fn activity_details(text: &str) -> Result<ActivityTemplate> {
    let (head, note) = text.split_once(',').unwrap_or((text, ""));
    let (head, shorthand_tags) = head_tags(head);
    let (description, explicit_tags) = note_tags(note);
    let (activity, category) = head.rsplit_once('@').unwrap_or((head, ""));
    let activity = activity.trim();
    if activity.is_empty() {
        return Err(invalid("an activity name is required"));
    }
    let description = description.trim();
    let mut tags = Vec::new();
    for tag in shorthand_tags.split('#') {
        add_tag(&mut tags, tag);
    }
    for word in description.split_whitespace() {
        let word = word.trim_start_matches(['(', '[', '{', '"', '\'']);
        if tag_start(word) {
            let tag = word[1..].trim_end_matches(['.', ',', '!', '?', ')', ']', '}']);
            add_tag(&mut tags, tag);
        }
    }
    for tag in explicit_tags.split(['#', ',']) {
        add_tag(&mut tags, tag);
    }
    Ok(ActivityTemplate {
        activity: activity.to_owned(),
        category: Some(category.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        description: description.to_owned(),
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn dt(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn quick_entry_and_inline_references() {
        let now = dt("2026-11-23 09:00");
        let entry = parse_entry(
            "-5 Fix #123@Work, check #bug and #123, #code review #bug",
            now,
            0,
        )
        .unwrap();
        assert_eq!(entry.start, dt("2026-11-23 08:55"));
        assert_eq!(entry.end, None);
        assert_eq!(entry.activity, "Fix #123");
        assert_eq!(entry.category.as_deref(), Some("Work"));
        assert_eq!(entry.description, "check #bug and #123");
        assert_eq!(entry.tags, ["bug", "code review"]);
        assert_eq!(
            parse_entry("Plan #code review #team", now, 0).unwrap().tags,
            ["code review", "team"]
        );
    }

    #[test]
    fn templates_identify_explicit_time_from_syntax_and_keep_quick_entry_fields() {
        for input in [
            "-0 Writing",
            "+0 Writing",
            "00:00 Writing",
            "0930 Writing",
            "9.30-10.00 Writing",
            "2026-11-23 09:00 Writing",
        ] {
            assert!(parse_activity_template(input).is_err(), "{input}");
        }
        let input =
            "  2.12.0 rollout@Work #client, replied to #123, contacted #qa, #release notes ";
        let template = parse_activity_template(input).unwrap();
        assert_eq!(template.activity, "2.12.0 rollout");
        assert_eq!(template.category.as_deref(), Some("Work"));
        assert_eq!(template.description, "replied to #123, contacted #qa");
        assert_eq!(template.tags, ["client", "qa", "release notes"]);
        assert_eq!(
            template,
            ActivityTemplate::from(parse_entry(input, dt("2026-11-23 09:00"), 0).unwrap())
        );
        assert_eq!(
            parse_activity_template("42 issues").unwrap().activity,
            "42 issues"
        );
    }

    #[test]
    fn shorthand_tags_do_not_swallow_notes_or_numeric_issue_references() {
        let now = dt("2026-11-23 09:00");
        let entry = parse_entry(
            "Work@Support #client #release notes, replied to #123 (see #bug), #QA",
            now,
            0,
        )
        .unwrap();
        assert_eq!(entry.activity, "Work");
        assert_eq!(entry.category.as_deref(), Some("Support"));
        assert_eq!(entry.description, "replied to #123 (see #bug)");
        assert_eq!(entry.tags, ["client", "release notes", "bug", "QA"]);

        let entry = parse_entry("Support, #client called, replied to #123, #QA", now, 0).unwrap();
        assert_eq!(entry.description, "#client called, replied to #123");
        assert_eq!(entry.tags, ["client", "QA"]);

        let entry = parse_entry("Support #mail, copied the literal string a,b,", now, 0).unwrap();
        assert_eq!(entry.description, "copied the literal string a,b,");

        let entry = parse_entry(
            "Support, addressed (#bug), contact x@y, incident #123",
            now,
            0,
        )
        .unwrap();
        assert_eq!(entry.tags, ["bug"]);
        assert_eq!(
            entry.description,
            "addressed (#bug), contact x@y, incident #123"
        );
    }

    #[test]
    fn hashes_and_commas_in_names_and_notes() {
        let now = dt("2026-11-23 09:00");
        let entry = parse_entry("#case@Category #2, first, then #review, #rešeno", now, 0).unwrap();
        assert_eq!(entry.activity, "#case");
        assert_eq!(entry.category.as_deref(), Some("Category #2"));
        assert_eq!(entry.description, "first, then #review");
        assert_eq!(entry.tags, ["review", "rešeno"]);
        assert_eq!(parse_entry("10.00@ABC", now, 0).unwrap().activity, "10.00");
    }

    #[test]
    fn resolves_relative_end_against_now_and_duration_against_start() {
        let now = dt("2026-11-23 09:00");
        for (text, end) in [
            ("-30 -10 Review", "2026-11-23 08:50"),
            ("-30 15 Review", "2026-11-23 08:45"),
            ("8:30 - 09:15 Review", "2026-11-23 09:15"),
        ] {
            let entry = parse_entry(text, now, 0).unwrap();
            assert_eq!(entry.start, dt("2026-11-23 08:30"));
            assert_eq!(entry.end, Some(dt(end)));
        }
    }

    #[test]
    fn date_and_clock_ranges_roll_over_midnight_and_workday_start() {
        let now = dt("2026-11-23 02:00");
        assert_eq!(
            parse_time("23:00", now, 300).unwrap(),
            dt("2026-11-22 23:00")
        );
        assert_eq!(
            parse_time("03:00", now, 300).unwrap(),
            dt("2026-11-23 03:00")
        );
        for (text, expected) in [
            ("2026-11-21 23:45-00:15 Night shift", "2026-11-22 00:15"),
            ("2026-11-21T04:30 06:00 Morning", "2026-11-21 06:00"),
        ] {
            assert_eq!(parse_entry(text, now, 300).unwrap().end, Some(dt(expected)));
        }
    }

    #[test]
    fn alternative_clock_spellings_preserve_activity_boundaries() {
        let now = dt("2026-11-23 09:00");
        for text in ["9.15", "09,15", "0915", "09:15"] {
            assert_eq!(parse_time(text, now, 0).unwrap(), dt("2026-11-23 09:15"));
        }
        let entry = parse_entry("2019-2025 Project", now, 0).unwrap();
        assert_eq!(entry.start, dt("2026-11-23 20:19"));
        assert_eq!(entry.end, Some(dt("2026-11-23 20:25")));
        for text in ["0915 -0945 Task", "2026-11-23 09.15 9,45 Task"] {
            let entry = parse_entry(text, now, 0).unwrap();
            assert_eq!(entry.start, dt("2026-11-23 09:15"));
            assert_eq!(entry.end, Some(dt("2026-11-23 09:45")));
        }
        let entry = parse_entry("2.12.0 rollout", now, 0).unwrap();
        assert_eq!(entry.activity, "2.12.0 rollout");
        assert_eq!(entry.start, now);
        assert!(parse_time("2.12.0", now, 0).is_err());
    }

    #[test]
    fn standalone_clock_distinguishes_dated_relative_and_invalid_inputs() {
        let expected = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        for value in ["09:30", "9.30", "9,30", " 0930 "] {
            assert_eq!(standalone_clock(value).unwrap(), Some(expected));
        }
        for value in ["2026-10-25 09:30", " NOW ", "-0", "+5"] {
            assert_eq!(standalone_clock(value).unwrap(), None);
        }
        for value in ["24:99", "09:30-10:30", "09:30 notes", "clock"] {
            assert!(standalone_clock(value).is_err());
        }
    }

    #[test]
    fn end_only_times_use_the_entry_day_and_start_reference() {
        let start = dt("2026-11-21 23:00");
        for input in ["00.15", "0015", "0:15", "0,15"] {
            assert_eq!(
                parse_end_time(input, start, 0).unwrap(),
                dt("2026-11-22 00:15")
            );
        }
        assert_eq!(
            parse_end_time("+10", start, 0).unwrap(),
            dt("2026-11-21 23:10")
        );
        assert!(parse_end_time("2026-11-21 22:30", start, 0).is_err());
        // An imported timestamp can retain seconds. An end in the same minute
        // must not accidentally create a 24-hour entry.
        assert!(parse_end_time("23:00", start.with_second(10).unwrap(), 0).is_err());
    }

    #[test]
    fn malformed_unicode_tokens_and_extreme_dates_return_results() {
        let now = dt("2026-11-23 09:00");
        for input in [
            "2026-11-🌀 09:00",
            "09:🍎",
            "9.１２",
            "０９１５",
            "+三",
            "09:16🐾",
        ] {
            assert!(parse_time(input, now, 0).is_err(), "{input}");
            // An unrecognized token may be a valid Unicode activity.
            let _ = parse_entry(input, now, 0);
        }
        assert!(parse_time("-1", NaiveDateTime::MIN, 0).is_err());
        assert!(parse_time("+1", NaiveDateTime::MAX, 0).is_err());
        assert!(parse_end_time("00:01", NaiveDateTime::MAX, 0).is_err());
    }

    #[test]
    fn rejects_invalid_inputs_and_overflows() {
        let now = dt("2026-11-23 09:00");
        for text in [
            "",
            "@Work",
            ", #tag",
            "12:7 Broken",
            "25:00-11:00 Broken",
            "2026-02-31 10:00 Broken",
            "-5 -30 Broken",
            "10:00- Broken",
            "10:00 -",
            "09:00 9223372036854775807 Broken",
            "-9999999999999999999999 Broken",
        ] {
            assert!(parse_entry(text, now, 0).is_err(), "{text}");
        }
        assert!(parse_time("09:00", now, 1440).is_err());
    }
}
