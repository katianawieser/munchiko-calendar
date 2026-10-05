//! Holidays and notable days (Milestone 3).
//!
//! The list lives in `data/holidays.toml`: one entry per holiday, each with a
//! *date rule* ("October 31", "4th Thursday of November", "47 days before
//! Easter"...). This module reads that file and works out the actual dates
//! for any year. It knows nothing about the database; `db::sync_holidays`
//! turns these dates into rows in the `events` table.
//!
//! Design notes:
//! - Reading happens in two steps. serde first fills `Raw...` structs that
//!   mirror the file exactly; then `validate` checks them and builds the real
//!   types. That lets us report problems in plain words ("Thanksgiving: n must
//!   be 1 to 5...") and means the rest of the app only sees valid rules.
//! - `deny_unknown_fields` turns a typo like `keyword = [...]` into an error
//!   instead of silently ignoring it.
//! - The default file is compiled into the binary (`include_str!`), so the app
//!   works even if `data/` is missing. If the file *is* there but broken, we
//!   report the error rather than quietly falling back, so an edit is never
//!   silently ignored.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Deserialize;

use crate::models::EventKind;

/// The copy of `data/holidays.toml` built into the app.
pub const BUILT_IN: &str = include_str!("../data/holidays.toml");

// ---------------------------------------------------------------------------
// The validated types the rest of the app uses
// ---------------------------------------------------------------------------

/// How to find a holiday's date in a given year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateRule {
    /// The same month and day every year. Feb 29 only happens in leap years.
    Fixed { month: u32, day: u32 },
    /// The nth `weekday` of `month`; a negative `n` counts from the end
    /// (-1 = last). Some years have no 5th Monday, and then there's no date.
    NthWeekday { n: i8, weekday: Weekday, month: u32 },
    /// Easter Sunday (use `offset_days` for days around it).
    Easter,
    /// Dates looked up by hand, for things no formula can predict.
    ByYear(BTreeMap<i32, NaiveDate>),
}

/// One entry from the holidays file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolidayRule {
    /// Also the holiday's identity when syncing with the database.
    pub name: String,
    pub description: String,
    pub kind: EventKind,
    pub keywords: Vec<String>,
    pub rule: DateRule,
    /// Days to move the date the rule gives (Black Friday = Thanksgiving + 1).
    pub offset_days: i64,
    /// How many days it lasts; 1 for a single day.
    pub days: u32,
}

/// When a holiday falls in one particular year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    pub start: NaiveDate,
    /// `None` for single-day holidays, matching `Event::end`.
    pub end: Option<NaiveDate>,
}

impl HolidayRule {
    /// The dates of this holiday in `year`, or `None` if it doesn't happen
    /// that year (no 5th Monday, no Feb 29, no `by_year` entry).
    pub fn occurrence(&self, year: i32) -> Option<Occurrence> {
        let base = match &self.rule {
            DateRule::Fixed { month, day } => NaiveDate::from_ymd_opt(year, *month, *day)?,
            DateRule::NthWeekday { n, weekday, month } => nth_weekday(year, *month, *weekday, *n)?,
            DateRule::Easter => easter_sunday(year)?,
            DateRule::ByYear(dates) => *dates.get(&year)?,
        };
        let start = base + Duration::days(self.offset_days);
        let end = (self.days > 1).then(|| start + Duration::days(i64::from(self.days) - 1));
        Some(Occurrence { start, end })
    }
}

// ---------------------------------------------------------------------------
// Date calculations
// ---------------------------------------------------------------------------

/// Easter Sunday in the Gregorian (Western) calendar, using the "Anonymous
/// Gregorian algorithm" (Meeus/Jones/Butcher). It's pure integer arithmetic:
/// roughly, find where the year sits in the 19-year cycle of moon phases, find
/// the first full moon on or after the spring equinox, and take the Sunday
/// after it. The tests check it against published Easter dates.
pub fn easter_sunday(year: i32) -> Option<NaiveDate> {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32)
}

/// The nth `weekday` of a month: n = 1 is the first, n = -1 the last.
/// Returns `None` if that month doesn't have one (e.g. a 5th Monday).
pub fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: i8) -> Option<NaiveDate> {
    if n > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        // Days from the 1st to the first matching weekday (0 to 6).
        let to_first =
            (7 + weekday.num_days_from_monday() - first.weekday().num_days_from_monday()) % 7;
        let date = first + Duration::days(i64::from(to_first) + 7 * (i64::from(n) - 1));
        (date.month() == month).then_some(date)
    } else {
        let last = last_day_of_month(year, month)?;
        // Days back from the last day to the last matching weekday (0 to 6).
        let to_last =
            (7 + last.weekday().num_days_from_monday() - weekday.num_days_from_monday()) % 7;
        let date = last - Duration::days(i64::from(to_last) + 7 * (i64::from(-n) - 1));
        (date.month() == month).then_some(date)
    }
}

fn last_day_of_month(year: i32, month: u32) -> Option<NaiveDate> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)?.pred_opt()
}

// ---------------------------------------------------------------------------
// Reading the file
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum HolidayError {
    /// The file couldn't be read from disk.
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file isn't valid TOML, or has a field we don't know.
    Toml(toml::de::Error),
    /// The TOML is fine but an entry doesn't make sense.
    Invalid { holiday: String, problem: String },
}

impl fmt::Display for HolidayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HolidayError::Io { path, error } => write!(f, "can't read {}: {error}", path.display()),
            HolidayError::Toml(e) => write!(f, "holidays file isn't valid: {e}"),
            HolidayError::Invalid { holiday, problem } => {
                write!(f, "holiday {holiday:?}: {problem}")
            }
        }
    }
}

impl std::error::Error for HolidayError {}

/// Where the holiday list came from, so the app can say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadedFrom {
    File(PathBuf),
    BuiltIn,
}

impl fmt::Display for LoadedFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadedFrom::File(path) => write!(f, "{}", path.display()),
            LoadedFrom::BuiltIn => write!(f, "the built-in list"),
        }
    }
}

/// Read the holidays file at `path`, or use the built-in list if there's no
/// file there. A file that exists but has a mistake is an error.
pub fn load(path: &Path) -> Result<(Vec<HolidayRule>, LoadedFrom), HolidayError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok((parse(&text)?, LoadedFrom::File(path.to_path_buf()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((parse(BUILT_IN)?, LoadedFrom::BuiltIn))
        }
        Err(error) => Err(HolidayError::Io {
            path: path.to_path_buf(),
            error,
        }),
    }
}

/// Parse and check the text of a holidays file.
pub fn parse(text: &str) -> Result<Vec<HolidayRule>, HolidayError> {
    let file: RawFile = toml::from_str(text).map_err(HolidayError::Toml)?;
    let mut seen = HashSet::new();
    file.holiday
        .into_iter()
        .map(|raw| {
            // The name is the holiday's identity, so it has to be unique.
            if !seen.insert(raw.name.clone()) {
                return Err(invalid(&raw.name, "appears more than once"));
            }
            validate(raw)
        })
        .collect()
}

/// The file exactly as written, before any checks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    holiday: Vec<RawHoliday>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHoliday {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    kind: RawKind,
    #[serde(default)]
    keywords: Vec<String>,
    date: Option<String>,
    nth_weekday: Option<RawNthWeekday>,
    easter_offset: Option<i64>,
    /// TOML table keys are always strings, so years arrive as "2027".
    by_year: Option<BTreeMap<String, String>>,
    #[serde(default)]
    offset_days: i64,
    #[serde(default = "one_day")]
    days: u32,
}

fn one_day() -> u32 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNthWeekday {
    n: i8,
    weekday: String,
    month: u32,
}

/// The `kind` labels allowed in the file. Kept here rather than as serde
/// attributes on `models::EventKind`, so the file format doesn't dictate how
/// the model is serialized elsewhere.
#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum RawKind {
    #[default]
    Holiday,
    NotableDay,
    Seasonal,
    Outing,
}

impl From<RawKind> for EventKind {
    fn from(kind: RawKind) -> Self {
        match kind {
            RawKind::Holiday => EventKind::Holiday,
            RawKind::NotableDay => EventKind::NotableDay,
            RawKind::Seasonal => EventKind::Seasonal,
            RawKind::Outing => EventKind::Outing,
        }
    }
}

fn invalid(holiday: &str, problem: impl Into<String>) -> HolidayError {
    HolidayError::Invalid {
        holiday: holiday.to_string(),
        problem: problem.into(),
    }
}

/// Parse "MM-DD", checking it's a real day of the year (Feb 29 allowed).
fn month_day(holiday: &str, text: &str) -> Result<(u32, u32), HolidayError> {
    let bad = || {
        invalid(
            holiday,
            format!("{text:?} isn't a date like \"10-31\" (MM-DD)"),
        )
    };
    let (m, d) = text.split_once('-').ok_or_else(bad)?;
    let (month, day) = (m.parse().map_err(|_| bad())?, d.parse().map_err(|_| bad())?);
    // 2000 was a leap year, so this accepts Feb 29 and rejects Feb 30.
    NaiveDate::from_ymd_opt(2000, month, day).ok_or_else(bad)?;
    Ok((month, day))
}

fn validate(raw: RawHoliday) -> Result<HolidayRule, HolidayError> {
    let name = raw.name.as_str();
    if name.trim().is_empty() {
        return Err(invalid(name, "name can't be empty"));
    }
    if raw.days == 0 {
        return Err(invalid(name, "days must be at least 1"));
    }

    let rules_given = [
        raw.date.is_some(),
        raw.nth_weekday.is_some(),
        raw.easter_offset.is_some(),
        raw.by_year.is_some(),
    ]
    .iter()
    .filter(|given| **given)
    .count();
    if rules_given != 1 {
        return Err(invalid(
            name,
            format!(
                "needs exactly one of date, nth_weekday, easter_offset or by_year (found {rules_given})"
            ),
        ));
    }

    let mut offset_days = raw.offset_days;
    let rule = if let Some(text) = &raw.date {
        let (month, day) = month_day(name, text)?;
        DateRule::Fixed { month, day }
    } else if let Some(nth) = &raw.nth_weekday {
        if !(1..=12).contains(&nth.month) {
            return Err(invalid(name, format!("month {} isn't 1 to 12", nth.month)));
        }
        if nth.n == 0 || !(-5..=5).contains(&nth.n) {
            return Err(invalid(
                name,
                "n must be 1 to 5, or -1 to -5 to count from the end",
            ));
        }
        let weekday: Weekday = nth.weekday.parse().map_err(|_| {
            invalid(
                name,
                format!("{:?} isn't a weekday like \"thu\"", nth.weekday),
            )
        })?;
        DateRule::NthWeekday {
            n: nth.n,
            weekday,
            month: nth.month,
        }
    } else if let Some(offset) = raw.easter_offset {
        // "47 days before Easter" is just Easter shifted by -47.
        offset_days += offset;
        DateRule::Easter
    } else {
        let mut dates = BTreeMap::new();
        for (year_text, md) in raw.by_year.iter().flatten() {
            let year: i32 = year_text
                .parse()
                .map_err(|_| invalid(name, format!("{year_text:?} isn't a year")))?;
            let (month, day) = month_day(name, md)?;
            let date = NaiveDate::from_ymd_opt(year, month, day)
                .ok_or_else(|| invalid(name, format!("{year} has no {md}")))?;
            dates.insert(year, date);
        }
        DateRule::ByYear(dates)
    };

    Ok(HolidayRule {
        name: raw.name,
        description: raw.description,
        kind: raw.kind.into(),
        keywords: raw.keywords,
        rule,
        offset_days,
        days: raw.days,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn one(text: &str) -> HolidayRule {
        parse(text).unwrap().remove(0)
    }

    fn start(text: &str, year: i32) -> Option<NaiveDate> {
        one(text).occurrence(year).map(|o| o.start)
    }

    #[test]
    fn easter_matches_published_dates() {
        let known = [
            (2008, 3, 23), // very early
            (2019, 4, 21),
            (2024, 3, 31),
            (2025, 4, 20),
            (2026, 4, 5),
            (2027, 3, 28),
            (2038, 4, 25), // very late
        ];
        for (y, m, day) in known {
            assert_eq!(easter_sunday(y), Some(d(y, m, day)), "Easter {y}");
        }
    }

    #[test]
    fn nth_weekday_counts_from_either_end() {
        // Thanksgiving: 4th Thursday of November.
        assert_eq!(
            nth_weekday(2026, 11, Weekday::Thu, 4),
            Some(d(2026, 11, 26))
        );
        assert_eq!(
            nth_weekday(2027, 11, Weekday::Thu, 4),
            Some(d(2027, 11, 25))
        );
        // Memorial Day: last Monday of May (May 31, 2026 is a Sunday).
        assert_eq!(nth_weekday(2026, 5, Weekday::Mon, -1), Some(d(2026, 5, 25)));
        // MLK Day: 3rd Monday of January.
        assert_eq!(nth_weekday(2027, 1, Weekday::Mon, 3), Some(d(2027, 1, 18)));
        // When the 1st is the weekday itself, it counts as the first one.
        assert_eq!(nth_weekday(2026, 11, Weekday::Sun, 1), Some(d(2026, 11, 1)));
        // February 2026 has only four Mondays.
        assert_eq!(nth_weekday(2026, 2, Weekday::Mon, 5), None);
        // December: the month-end calculation has to roll into next year.
        assert_eq!(
            nth_weekday(2026, 12, Weekday::Thu, -1),
            Some(d(2026, 12, 31))
        );
    }

    #[test]
    fn each_rule_type_finds_its_date() {
        let fixed = "[[holiday]]\nname = \"Halloween\"\ndate = \"10-31\"";
        assert_eq!(start(fixed, 2026), Some(d(2026, 10, 31)));

        let mardi_gras = "[[holiday]]\nname = \"Mardi Gras\"\neaster_offset = -47";
        assert_eq!(start(mardi_gras, 2026), Some(d(2026, 2, 17)));

        let black_friday = r#"
            [[holiday]]
            name = "Black Friday"
            nth_weekday = { n = 4, weekday = "thu", month = 11 }
            offset_days = 1
        "#;
        assert_eq!(start(black_friday, 2026), Some(d(2026, 11, 27)));

        let lunar = "[[holiday]]\nname = \"Lunar New Year\"\nby_year = { 2027 = \"02-06\" }";
        assert_eq!(start(lunar, 2027), Some(d(2027, 2, 6)));
        assert_eq!(start(lunar, 2028), None, "years without a date are skipped");

        let leap = "[[holiday]]\nname = \"Leap Day\"\ndate = \"02-29\"";
        assert_eq!(start(leap, 2028), Some(d(2028, 2, 29)));
        assert_eq!(start(leap, 2027), None);
    }

    #[test]
    fn multi_day_events_get_an_end_date() {
        let rule = one(r#"
            [[holiday]]
            name = "Twelve Days of Christmas"
            kind = "seasonal"
            date = "12-25"
            days = 12
            "#);
        assert_eq!(rule.kind, EventKind::Seasonal);
        assert_eq!(
            rule.occurrence(2026),
            Some(Occurrence {
                start: d(2026, 12, 25),
                end: Some(d(2027, 1, 5))
            })
        );

        let single = one("[[holiday]]\nname = \"Pi Day\"\ndate = \"03-14\"");
        assert_eq!(single.kind, EventKind::Holiday, "kind defaults to holiday");
        assert_eq!(single.occurrence(2027).unwrap().end, None);
    }

    #[test]
    fn mistakes_are_reported_by_name() {
        let cases = [
            ("[[holiday]]\nname = \"X\"", "exactly one"),
            (
                "[[holiday]]\nname = \"X\"\ndate = \"10-31\"\neaster_offset = 1",
                "exactly one",
            ),
            ("[[holiday]]\nname = \"X\"\ndate = \"02-30\"", "MM-DD"),
            ("[[holiday]]\nname = \"X\"\ndate = \"Oct 31\"", "MM-DD"),
            (
                "[[holiday]]\nname = \"X\"\nnth_weekday = { n = 0, weekday = \"mon\", month = 1 }",
                "n must be",
            ),
            (
                "[[holiday]]\nname = \"X\"\nnth_weekday = { n = 1, weekday = \"funday\", month = 1 }",
                "weekday",
            ),
            (
                "[[holiday]]\nname = \"X\"\ndate = \"01-01\"\ndays = 0",
                "at least 1",
            ),
            (
                "[[holiday]]\nname = \"X\"\ndate = \"01-01\"\n[[holiday]]\nname = \"X\"\ndate = \"01-02\"",
                "more than once",
            ),
        ];
        for (text, expected) in cases {
            let err = parse(text).unwrap_err().to_string();
            assert!(err.contains(expected), "{text:?} gave {err:?}");
            assert!(
                err.contains("\"X\""),
                "error should name the holiday: {err:?}"
            );
        }

        // A typo in a field name is caught too, not silently ignored.
        let typo = "[[holiday]]\nname = \"X\"\ndate = \"01-01\"\nkeyword = [\"a\"]";
        assert!(matches!(parse(typo), Err(HolidayError::Toml(_))));
    }

    #[test]
    fn built_in_list_is_valid() {
        let rules = parse(BUILT_IN).unwrap();
        assert!(rules.len() >= 35, "only {} holidays", rules.len());

        let find = |name: &str| rules.iter().find(|r| r.name == name).unwrap();
        assert_eq!(
            find("Thanksgiving").occurrence(2026).unwrap().start,
            d(2026, 11, 26)
        );
        assert_eq!(
            find("Easter").occurrence(2027).unwrap().start,
            d(2027, 3, 28)
        );
        assert_eq!(
            find("Mochi's Birthday").occurrence(2026).unwrap().start,
            d(2026, 10, 15)
        );
        assert_eq!(
            find("Momo's Adoption Day").occurrence(2027).unwrap().start,
            d(2027, 7, 5)
        );
        // First full week of May 2027: Sunday May 2 to Saturday May 8.
        assert_eq!(
            find("National Pet Week").occurrence(2027),
            Some(Occurrence {
                start: d(2027, 5, 2),
                end: Some(d(2027, 5, 8))
            })
        );
    }

    #[test]
    fn missing_file_falls_back_to_built_in() {
        let (rules, from) = load(Path::new("no/such/holidays.toml")).unwrap();
        assert_eq!(from, LoadedFrom::BuiltIn);
        assert_eq!(rules, parse(BUILT_IN).unwrap());
    }
}
