//! Example data: the Halloween post from Milestone 1, saved through the
//! repository. The app adds it on first run so there's something to look at,
//! and the web tests use it as a realistic fixture.

use std::error::Error;

use crate::db::{NewAccessory, NewFabric, NewPostPlan, Repository};
use crate::models::*;

/// Save the Halloween example and attach it to `year`'s Halloween, which must
/// already be on the calendar (run `sync_holidays` first).
pub async fn seed_halloween(db: &Repository, year: i32) -> Result<PostPlanId, Box<dyn Error>> {
    let halloween = db
        .find_holiday("Halloween", year)
        .await?
        .ok_or("the holidays file has no Halloween")?;

    let velvet = db
        .create_fabric(&NewFabric {
            name: "Orange velvet".into(),
            description: "Soft pumpkin-orange velvet".into(),
            colors: vec!["orange".into()],
            keywords: vec!["halloween".into(), "autumn".into(), "pumpkin".into()],
            in_stock: false,
        })
        .await?;
    let satin = db
        .create_fabric(&NewFabric {
            name: "Black satin".into(),
            description: "Shiny black satin".into(),
            colors: vec!["black".into()],
            keywords: vec!["halloween".into(), "formal".into()],
            in_stock: true,
        })
        .await?;
    db.pair_fabrics(velvet, satin).await?;

    let pumpkin = db
        .create_accessory(&NewAccessory {
            kind: AccessoryKind::Bow,
            name: "Pumpkin bow".into(),
            description: "Orange velvet with a black satin middle".into(),
            colors: vec!["orange".into(), "black".into()],
            fabrics: vec![
                FabricUse {
                    fabric_id: velvet,
                    role: FabricRole::Main,
                },
                FabricUse {
                    fabric_id: satin,
                    role: FabricRole::Middle,
                },
            ],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            make_days: None,
        })
        .await?;
    let bat = db
        .create_accessory(&NewAccessory {
            kind: AccessoryKind::Bow,
            name: "Bat bow".into(),
            description: "Black satin bat-wing bow".into(),
            colors: vec!["black".into()],
            fabrics: vec![FabricUse {
                fabric_id: satin,
                role: FabricRole::Main,
            }],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            make_days: Some(2),
        })
        .await?;
    db.pair_accessories(pumpkin, bat).await?;

    let ghost = db
        .create_accessory(&NewAccessory {
            kind: AccessoryKind::Scarf,
            name: "Ghost scarf".into(),
            description: "Glow-in-the-dark ghost print".into(),
            colors: vec!["white".into()],
            fabrics: vec![],
            source: Source::Bought { brand: None },
            status: AccessoryStatus::Idea,
            make_days: None,
        })
        .await?;

    let plan = db
        .create_post_plan(&NewPostPlan {
            event_id: halloween.id,
            post_date: halloween.start,
            looks: vec![
                Look {
                    pet: Pet::Mochi,
                    accessory_id: pumpkin,
                },
                Look {
                    pet: Pet::Mochi,
                    accessory_id: ghost,
                },
                Look {
                    pet: Pet::Momo,
                    accessory_id: bat,
                },
            ],
            backdrop: "hay bales and mini pumpkins".into(),
            notes: String::new(),
        })
        .await?;
    Ok(plan)
}
