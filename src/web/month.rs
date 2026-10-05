//! Laying out a month calendar: which days are on the grid, and what goes in
//! each day. This is plain logic with no web code, so it's unit tested like
//! the planner; the template just draws what this produces.
//!
//! The grid always runs Sunday to Saturday, so it starts on the Sunday on or
//! before the 1st and ends on the Saturday on or after the last day. The days
//! from the neighboring months are shown greyed out.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::models::{Event, EventKind, PostPlanId};
use crate::planner::{TaskKind, TaskStatus, Timeline};

/// A post plan whose schedule has been worked out, ready to draw.
#[derive(Debug, Clone, Copy)]
pub struct PlannedPost<'a> {
    pub id: PostPlanId,
    pub event_name: &'a str,
    pub timeline: &'a Timeline,
}

impl PlannedPost<'_> {
    fn href(&self) -> String {
        format!("/posts/{}", self.id.0)
    }

    /// A post whose publish date has passed is finished, so its tasks are
    /// history, not overdue work.
    fn is_finished(&self, today: NaiveDate) -> bool {
        self.timeline.post_date < today
    }
}

/// One line inside a day cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub icon: &'static str,
    pub label: String,
    /// CSS class that picks the color, e.g. "holiday" or "task-make".
    pub class: &'static str,
    pub href: Option<String>,
    pub overdue: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    pub date: NaiveDate,
    pub number: u32,
    /// False for the greyed-out days from the previous or next month.
    pub in_month: bool,
    pub is_today: bool,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthView {
    pub title: String,
    pub prev_href: String,
    pub prev_label: String,
    pub next_href: String,
    pub next_label: String,
    /// Each week is Sunday to Saturday.
    pub weeks: Vec<Vec<Day>>,
}

/// The first and last day shown on the grid for a month, or `None` if the
/// year/month doesn't exist.
pub fn grid_range(year: i32, month: u32) -> Option<(NaiveDate, NaiveDate)> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let (next_year, next_month) = next_month(year, month);
    let last = NaiveDate::from_ymd_opt(next_year, next_month, 1)?.pred_opt()?;

    let start = first - Duration::days(i64::from(first.weekday().num_days_from_sunday()));
    let end = last + Duration::days(i64::from(6 - last.weekday().num_days_from_sunday()));
    Some((start, end))
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn prev_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES[(month - 1) as usize]
}

fn event_style(kind: EventKind) -> (&'static str, &'static str) {
    match kind {
        EventKind::Holiday => ("🎉", "holiday"),
        EventKind::NotableDay => ("⭐", "notable"),
        EventKind::Seasonal => ("🌿", "seasonal"),
        EventKind::Outing => ("🚗", "outing"),
    }
}

fn task_style(kind: TaskKind) -> (&'static str, &'static str) {
    match kind {
        TaskKind::BuyFabric => ("🧵", "task-fabric"),
        TaskKind::OrderAccessory => ("📦", "task-order"),
        TaskKind::MakeAccessory => ("✂️", "task-make"),
        TaskKind::BuildBackdrop => ("🎨", "task-backdrop"),
        TaskKind::PhotoShoot => ("📷", "task-shoot"),
        TaskKind::Publish => ("📤", "post"),
    }
}

/// Short label for a calendar cell. Shoot and publish days name the post,
/// since several posts can be in progress at once.
fn task_label(kind: TaskKind, title: &str, event_name: &str) -> String {
    match kind {
        TaskKind::PhotoShoot => format!("Shoot: {event_name}"),
        TaskKind::Publish => format!("Post: {event_name}"),
        _ => title.to_string(),
    }
}

/// Lay out `year`/`month`. `events` and `posts` may include things outside
/// the grid; they're simply skipped. Within a day, events come first, then
/// tasks in the order they happen.
pub fn month_view(
    year: i32,
    month: u32,
    today: NaiveDate,
    events: &[Event],
    posts: &[PlannedPost<'_>],
) -> Option<MonthView> {
    let (start, end) = grid_range(year, month)?;

    let mut weeks: Vec<Vec<Day>> = Vec::new();
    let mut date = start;
    while date <= end {
        if date.weekday() == Weekday::Sun {
            weeks.push(Vec::with_capacity(7));
        }
        let mut entries = Vec::new();

        for event in events
            .iter()
            .filter(|e| e.start <= date && date <= e.end_date())
        {
            let (icon, class) = event_style(event.kind);
            entries.push(Entry {
                icon,
                label: event.name.clone(),
                class,
                href: Some(format!("/events/{}", event.id.0)),
                overdue: false,
            });
        }

        for post in posts {
            for task in post
                .timeline
                .tasks
                .iter()
                .filter(|t| t.start <= date && date <= t.due)
            {
                let (icon, class) = task_style(task.kind);
                let overdue = !post.is_finished(today)
                    && matches!(task.status(today), TaskStatus::Overdue { .. });
                entries.push(Entry {
                    icon,
                    label: task_label(task.kind, &task.title, post.event_name),
                    class,
                    href: Some(post.href()),
                    overdue,
                });
            }
        }

        weeks
            .last_mut()
            .expect("grid starts on a Sunday")
            .push(Day {
                date,
                number: date.day(),
                in_month: date.month() == month,
                is_today: date == today,
                entries,
            });
        date += Duration::days(1);
    }

    let (py, pm) = prev_month(year, month);
    let (ny, nm) = next_month(year, month);
    Some(MonthView {
        title: format!("{} {year}", month_name(month)),
        prev_href: format!("/calendar/{py}/{pm}"),
        prev_label: month_name(pm).to_string(),
        next_href: format!("/calendar/{ny}/{nm}"),
        next_label: month_name(nm).to_string(),
        weeks,
    })
}

/// A task in the "what to do now" box above the calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaItem {
    pub label: String,
    pub when: String,
    pub href: String,
    pub overdue: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agenda {
    /// Tasks you should be working on today, including overdue ones.
    pub now: Vec<AgendaItem>,
    /// The next task to start, if nothing is due right now.
    pub next: Option<AgendaItem>,
}

/// What needs doing today, across every unfinished post.
pub fn agenda(today: NaiveDate, posts: &[PlannedPost<'_>]) -> Agenda {
    let mut agenda = Agenda::default();
    let mut next: Option<(NaiveDate, AgendaItem)> = None;

    for post in posts.iter().filter(|p| !p.is_finished(today)) {
        for task in &post.timeline.tasks {
            let label = task_label(task.kind, &task.title, post.event_name);
            match task.status(today) {
                TaskStatus::Active => agenda.now.push(AgendaItem {
                    label,
                    when: format!("due {}", task.due.format("%b %-d")),
                    href: post.href(),
                    overdue: false,
                }),
                TaskStatus::Overdue { days_late } => agenda.now.push(AgendaItem {
                    label,
                    when: format!("{days_late}d late"),
                    href: post.href(),
                    overdue: true,
                }),
                TaskStatus::Upcoming { starts_in_days } => {
                    if next.as_ref().is_none_or(|(start, _)| task.start < *start) {
                        let item = AgendaItem {
                            label,
                            when: format!(
                                "starts in {starts_in_days}d ({})",
                                task.start.format("%b %-d")
                            ),
                            href: post.href(),
                            overdue: false,
                        };
                        next = Some((task.start, item));
                    }
                }
            }
        }
    }

    // Overdue first, then by label for a stable order.
    agenda
        .now
        .sort_by(|a, b| b.overdue.cmp(&a.overdue).then(a.label.cmp(&b.label)));
    agenda.next = next.map(|(_, item)| item);
    agenda
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EventId;
    use crate::planner::PlanTask;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn event(name: &str, start: NaiveDate, end: Option<NaiveDate>) -> Event {
        Event {
            id: EventId(1),
            name: name.into(),
            description: String::new(),
            kind: EventKind::Holiday,
            start,
            end,
            keywords: vec![],
            from_holidays_file: true,
        }
    }

    fn task(kind: TaskKind, title: &str, start: NaiveDate, due: NaiveDate) -> PlanTask {
        PlanTask {
            kind,
            title: title.into(),
            start,
            due,
            accessory_id: None,
            fabric_id: None,
        }
    }

    /// A tiny Halloween timeline: make a bow Oct 24-27, publish Oct 31.
    fn halloween() -> Timeline {
        Timeline {
            post_date: d(2026, 10, 31),
            shoot_date: d(2026, 10, 28),
            ready_by: d(2026, 10, 27),
            tasks: vec![
                task(
                    TaskKind::MakeAccessory,
                    "Make Pumpkin bow",
                    d(2026, 10, 24),
                    d(2026, 10, 27),
                ),
                task(
                    TaskKind::Publish,
                    "Publish the post",
                    d(2026, 10, 31),
                    d(2026, 10, 31),
                ),
            ],
        }
    }

    fn day(view: &MonthView, date: NaiveDate) -> &Day {
        view.weeks
            .iter()
            .flatten()
            .find(|day| day.date == date)
            .unwrap()
    }

    #[test]
    fn grid_runs_sunday_to_saturday() {
        // Oct 1, 2026 is a Thursday and Oct 31 a Saturday.
        assert_eq!(
            grid_range(2026, 10),
            Some((d(2026, 9, 27), d(2026, 10, 31)))
        );
        // Feb 2026 starts on a Sunday and ends on a Saturday: exactly 4 weeks.
        assert_eq!(grid_range(2026, 2), Some((d(2026, 2, 1), d(2026, 2, 28))));
        // December's grid runs into January of the next year.
        assert_eq!(grid_range(2026, 12), Some((d(2026, 11, 29), d(2027, 1, 2))));
        assert_eq!(grid_range(2026, 13), None);
    }

    #[test]
    fn view_has_whole_weeks_and_marks_other_months() {
        let view = month_view(2026, 10, d(2026, 10, 5), &[], &[]).unwrap();
        assert_eq!(view.title, "October 2026");
        assert_eq!(view.weeks.len(), 5);
        assert!(view.weeks.iter().all(|week| week.len() == 7));
        assert!(!day(&view, d(2026, 9, 30)).in_month);
        assert!(day(&view, d(2026, 10, 1)).in_month);
        assert!(day(&view, d(2026, 10, 5)).is_today);
    }

    #[test]
    fn prev_and_next_wrap_around_the_year() {
        let jan = month_view(2027, 1, d(2027, 1, 1), &[], &[]).unwrap();
        assert_eq!(jan.prev_href, "/calendar/2026/12");
        assert_eq!(jan.prev_label, "December");
        let dec = month_view(2026, 12, d(2026, 12, 1), &[], &[]).unwrap();
        assert_eq!(dec.next_href, "/calendar/2027/1");
    }

    #[test]
    fn multi_day_events_appear_on_every_day_they_cover() {
        // Runs from November into December.
        let events = [event("Long weekend", d(2026, 11, 29), Some(d(2026, 12, 2)))];
        let view = month_view(2026, 12, d(2026, 12, 1), &events, &[]).unwrap();
        for date in [
            d(2026, 11, 29),
            d(2026, 11, 30),
            d(2026, 12, 1),
            d(2026, 12, 2),
        ] {
            assert_eq!(day(&view, date).entries[0].label, "Long weekend", "{date}");
        }
        assert!(day(&view, d(2026, 12, 3)).entries.is_empty());
    }

    #[test]
    fn tasks_fill_their_window_and_link_to_the_post() {
        let timeline = halloween();
        let posts = [PlannedPost {
            id: PostPlanId(7),
            event_name: "Halloween",
            timeline: &timeline,
        }];
        let events = [event("Halloween", d(2026, 10, 31), None)];
        let view = month_view(2026, 10, d(2026, 10, 5), &events, &posts).unwrap();

        let make_days: Vec<u32> = view
            .weeks
            .iter()
            .flatten()
            .filter(|day| day.entries.iter().any(|e| e.class == "task-make"))
            .map(|day| day.number)
            .collect();
        assert_eq!(make_days, vec![24, 25, 26, 27]);

        // Halloween itself: the holiday first, then the post.
        let labels: Vec<&str> = day(&view, d(2026, 10, 31))
            .entries
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Halloween", "Post: Halloween"]);
        assert_eq!(
            day(&view, d(2026, 10, 31)).entries[1].href.as_deref(),
            Some("/posts/7")
        );
    }

    #[test]
    fn overdue_only_while_the_post_is_still_ahead() {
        let timeline = halloween();
        let posts = [PlannedPost {
            id: PostPlanId(1),
            event_name: "Halloween",
            timeline: &timeline,
        }];

        // Oct 29: the bow was due Oct 27 and the post is still to come.
        let view = month_view(2026, 10, d(2026, 10, 29), &[], &posts).unwrap();
        assert!(day(&view, d(2026, 10, 27)).entries[0].overdue);

        // In November the post is done, so nothing is "overdue" any more.
        let view = month_view(2026, 10, d(2026, 11, 5), &[], &posts).unwrap();
        assert!(!day(&view, d(2026, 10, 27)).entries[0].overdue);
    }

    #[test]
    fn agenda_shows_current_work_or_whats_next() {
        let timeline = halloween();
        let posts = [PlannedPost {
            id: PostPlanId(1),
            event_name: "Halloween",
            timeline: &timeline,
        }];

        let early = agenda(d(2026, 10, 5), &posts);
        assert!(early.now.is_empty());
        let next = early.next.unwrap();
        assert_eq!(next.label, "Make Pumpkin bow");
        assert_eq!(next.when, "starts in 19d (Oct 24)");

        let busy = agenda(d(2026, 10, 25), &posts);
        assert_eq!(busy.now.len(), 1);
        assert_eq!(busy.now[0].when, "due Oct 27");

        let late = agenda(d(2026, 10, 29), &posts);
        assert!(late.now[0].overdue);
        assert_eq!(late.now[0].when, "2d late");

        assert_eq!(
            agenda(d(2026, 11, 1), &posts),
            Agenda::default(),
            "finished posts drop off"
        );
    }
}
