//! "Done" buttons for prep tasks, on the calendar's to-do box and on post
//! and series timelines. A task isn't stored anywhere: it's worked out from
//! the accessory or fabric it's about. So "done" means changing that item
//! (the bow is Ready, the fabric is in the stash), and the planner then
//! simply stops producing the task.
//!
//! The buttons post to the item's own routes (`/accessories/{id}/status`,
//! `/fabrics/{id}/stock`) with a `back` address to return to.

use crate::planner::{PlanTask, TaskKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoneButton {
    pub action: String,
    /// The one form field the route needs, e.g. `status` = `ready`.
    pub field: &'static str,
    pub value: &'static str,
    pub label: &'static str,
}

/// The button for `task`, if finishing it is something the app records.
/// Backdrops, shoots and publishing aren't tracked, so they have none.
pub fn for_task(task: &PlanTask) -> Option<DoneButton> {
    let ready = |label| {
        task.accessory_id.map(|id| DoneButton {
            action: format!("/accessories/{}/status", id.0),
            field: "status",
            value: "ready",
            label,
        })
    };
    match task.kind {
        TaskKind::MakeAccessory => ready("✓ Made it"),
        TaskKind::OrderAccessory => ready("✓ It arrived"),
        TaskKind::BuyFabric => task.fabric_id.map(|id| DoneButton {
            action: format!("/fabrics/{}/stock", id.0),
            // Set, not toggle: clicking twice (or from an old tab) can't
            // put it back on the shopping list.
            field: "in_stock",
            value: "1",
            label: "✓ Bought it",
        }),
        TaskKind::BuildBackdrop | TaskKind::PhotoShoot | TaskKind::Publish => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccessoryId, FabricId};
    use chrono::NaiveDate;

    fn task(kind: TaskKind) -> PlanTask {
        let day = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        PlanTask {
            kind,
            title: "x".into(),
            start: day,
            due: day,
            accessory_id: Some(AccessoryId(4)),
            fabric_id: Some(FabricId(7)),
        }
    }

    #[test]
    fn only_tracked_tasks_get_a_button() {
        let make = for_task(&task(TaskKind::MakeAccessory)).unwrap();
        assert_eq!(make.action, "/accessories/4/status");
        assert_eq!((make.field, make.value), ("status", "ready"));
        assert_eq!(
            for_task(&task(TaskKind::OrderAccessory)).unwrap().label,
            "✓ It arrived"
        );
        let buy = for_task(&task(TaskKind::BuyFabric)).unwrap();
        assert_eq!(buy.action, "/fabrics/7/stock");
        assert_eq!((buy.field, buy.value), ("in_stock", "1"));
        for kind in [
            TaskKind::BuildBackdrop,
            TaskKind::PhotoShoot,
            TaskKind::Publish,
        ] {
            assert_eq!(for_task(&task(kind)), None);
        }
    }
}
