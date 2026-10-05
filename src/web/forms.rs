//! Helpers for HTML forms: reading what the browser sent, and describing the
//! choices in a `<select>` or a list of checkboxes.
//!
//! A browser sends a form as `name=value` pairs, and a group of checkboxes
//! sends the same name once per ticked box (`pairs=1&pairs=3`). Axum's `Form`
//! extractor can hand us those pairs as a plain list, and [`FormData`] reads
//! from it. That avoids an extra dependency just for repeated fields.

use chrono::NaiveDate;

use crate::models::{AccessoryKind, AccessoryStatus, EventKind, FabricRole, Pet};

/// A field that should have held a number didn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotANumber;

/// A date field held something that isn't a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotADate;

/// The fields of one form submission.
#[derive(Debug, Clone, Default)]
pub struct FormData(Vec<(String, String)>);

impl FormData {
    pub fn new(pairs: Vec<(String, String)>) -> Self {
        Self(pairs)
    }

    /// The field's value with surrounding spaces trimmed; empty if missing.
    pub fn text(&self, name: &str) -> String {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default()
    }

    /// A checkbox: browsers only send it when it's ticked.
    pub fn checked(&self, name: &str) -> bool {
        self.0.iter().any(|(key, _)| key == name)
    }

    /// Every value sent for `name`, e.g. all ticked checkboxes in a group.
    pub fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .filter(move |(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// A date from an `<input type="date">`, which always sends YYYY-MM-DD.
    /// `Ok(None)` if left empty.
    pub fn optional_date(&self, name: &str) -> Result<Option<NaiveDate>, NotADate> {
        let text = self.text(name);
        if text.is_empty() {
            Ok(None)
        } else {
            NaiveDate::parse_from_str(&text, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| NotADate)
        }
    }

    /// An optional id from a `<select>`: the "none" choice sends "".
    pub fn optional_id(&self, name: &str) -> Result<Option<i64>, NotANumber> {
        let text = self.text(name);
        if text.is_empty() {
            Ok(None)
        } else {
            text.parse().map(Some).map_err(|_| NotANumber)
        }
    }
}

/// "Orange, black , ,Gold" → ["orange", "black", "gold"]. Lowercase so that
/// matching keywords and colors later (M5) doesn't depend on capitals.
pub fn split_list(text: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for item in text.split(',').map(|s| s.trim().to_lowercase()) {
        if !item.is_empty() && !items.contains(&item) {
            items.push(item);
        }
    }
    items
}

/// The reverse of [`split_list`], for filling in an edit form.
pub fn join_list(items: &[String]) -> String {
    items.join(", ")
}

/// One choice in a `<select>` or one checkbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

impl Choice {
    pub fn new(value: impl ToString, label: impl Into<String>, selected: bool) -> Self {
        Self {
            value: value.to_string(),
            label: label.into(),
            selected,
        }
    }
}

// The values the forms use for each enum, and how to show them. One table
// per enum serves both directions (drawing the form and reading it back).

pub const KINDS: [(AccessoryKind, &str, &str); 3] = [
    (AccessoryKind::Bow, "bow", "Bow"),
    (AccessoryKind::Scarf, "scarf", "Scarf"),
    (AccessoryKind::Costume, "costume", "Costume"),
];

pub const STATUSES: [(AccessoryStatus, &str, &str); 3] = [
    (AccessoryStatus::Idea, "idea", "Idea"),
    (AccessoryStatus::InProgress, "in_progress", "In progress"),
    (AccessoryStatus::Ready, "ready", "Ready"),
];

pub const EVENT_KINDS: [(EventKind, &str, &str); 4] = [
    (EventKind::Outing, "outing", "Outing"),
    (EventKind::Holiday, "holiday", "Holiday"),
    (EventKind::NotableDay, "notable_day", "Notable day"),
    (EventKind::Seasonal, "seasonal", "Season"),
];

/// Each pet has its own group of checkboxes on the post form.
pub const PETS: [(Pet, &str, &str); 2] =
    [(Pet::Mochi, "mochi", "Mochi"), (Pet::Momo, "momo", "Momo")];

/// Each fabric role has its own `<select>` on the accessory form.
pub const ROLES: [(FabricRole, &str, &str); 3] = [
    (FabricRole::Main, "main_fabric", "Main fabric"),
    (FabricRole::Middle, "middle_fabric", "Middle fabric"),
    (FabricRole::Accent, "accent_fabric", "Accent fabric"),
];

/// Look up an enum by its form value.
pub fn parse_choice<T: Copy>(table: &[(T, &str, &str)], value: &str) -> Option<T> {
    table
        .iter()
        .find(|(_, v, _)| *v == value)
        .map(|(t, _, _)| *t)
}

/// The display label for an enum value.
pub fn label_of<T: PartialEq>(table: &[(T, &'static str, &'static str)], item: T) -> &'static str {
    table
        .iter()
        .find(|(t, _, _)| *t == item)
        .map(|(_, _, label)| *label)
        .unwrap_or("?")
}

/// Choices for a `<select>` built from one of the tables above.
pub fn choices<T: PartialEq>(table: &[(T, &str, &str)], current: T) -> Vec<Choice> {
    table
        .iter()
        .map(|(t, value, label)| Choice::new(value, *label, *t == current))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(pairs: &[(&str, &str)]) -> FormData {
        FormData::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn reads_text_checkboxes_and_repeated_fields() {
        let f = form(&[
            ("name", "  Velvet "),
            ("in_stock", "on"),
            ("pairs", "1"),
            ("pairs", "3"),
        ]);
        assert_eq!(f.text("name"), "Velvet");
        assert_eq!(f.text("missing"), "");
        assert!(f.checked("in_stock"));
        assert!(!f.checked("other"));
        assert_eq!(f.all("pairs").collect::<Vec<_>>(), vec!["1", "3"]);
    }

    #[test]
    fn optional_ids_allow_none() {
        let f = form(&[("a", ""), ("b", "7"), ("c", "seven")]);
        assert_eq!(f.optional_id("a"), Ok(None));
        assert_eq!(f.optional_id("b"), Ok(Some(7)));
        assert_eq!(f.optional_id("c"), Err(NotANumber));
    }

    #[test]
    fn dates_come_from_date_inputs() {
        let f = form(&[
            ("a", "2026-10-31"),
            ("b", ""),
            ("c", "10/31/2026"),
            ("d", "2026-02-30"),
        ]);
        assert_eq!(
            f.optional_date("a"),
            Ok(NaiveDate::from_ymd_opt(2026, 10, 31))
        );
        assert_eq!(f.optional_date("b"), Ok(None));
        assert_eq!(f.optional_date("c"), Err(NotADate));
        assert_eq!(f.optional_date("d"), Err(NotADate));
    }

    #[test]
    fn lists_are_trimmed_lowercased_and_deduplicated() {
        assert_eq!(
            split_list("Orange, black , ,Gold, orange"),
            vec!["orange", "black", "gold"]
        );
        assert!(split_list("  ").is_empty());
        assert_eq!(join_list(&split_list("a,b")), "a, b");
    }

    #[test]
    fn enum_tables_work_both_ways() {
        assert_eq!(parse_choice(&KINDS, "scarf"), Some(AccessoryKind::Scarf));
        assert_eq!(parse_choice(&KINDS, "hat"), None);
        assert_eq!(
            label_of(&STATUSES, AccessoryStatus::InProgress),
            "In progress"
        );
        let selected: Vec<bool> = choices(&KINDS, AccessoryKind::Bow)
            .iter()
            .map(|c| c.selected)
            .collect();
        assert_eq!(selected, vec![true, false, false]);
    }
}
