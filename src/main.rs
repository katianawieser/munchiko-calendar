//! Starts the Munchiko Calendar web app.
//!
//! On startup it opens the database (running any new migrations), syncs this
//! year's and next year's holidays from `data/holidays.toml`, adds the
//! Halloween example if there are no posts yet, and serves the app at
//! http://127.0.0.1:3000 until you press Ctrl+C.

use std::error::Error;
use std::path::Path;

use chrono::{Datelike, Local};
use munchiko_calendar::db::Repository;
use munchiko_calendar::web::{AppState, router};
use munchiko_calendar::{demo, holidays};

/// Where the data lives unless `DATABASE_URL` says otherwise.
const DEFAULT_DATABASE_URL: &str = "sqlite:munchiko.db";

/// The editable holidays list (the built-in copy is used if it's missing).
const HOLIDAYS_FILE: &str = "data/holidays.toml";

/// Only this computer can reach the app; there's no login.
const DEFAULT_ADDRESS: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.into());
    let db = Repository::connect(&url).await?;

    // Keep this year's and next year's holidays in step with the file.
    let (rules, loaded_from) = holidays::load(Path::new(HOLIDAYS_FILE))?;
    let this_year = Local::now().date_naive().year();
    let sync = db.sync_holidays(&rules, this_year..=this_year + 1).await?;
    println!(
        "Holidays from {loaded_from}: {} added, {} updated, {} removed",
        sync.added, sync.updated, sync.removed
    );
    if sync.kept_for_plans > 0 {
        println!(
            "  ({} no longer in the file but kept because a post is planned)",
            sync.kept_for_plans
        );
    }

    // Only a brand-new database gets the example. Checking for "no posts"
    // isn't enough: once you clear the examples out, they'd come back.
    if !db.has_user_data().await? {
        demo::seed_halloween(&db, this_year).await?;
        println!("New database: added the Halloween example (delete it any time)");
    }

    let address = std::env::var("ADDRESS").unwrap_or_else(|_| DEFAULT_ADDRESS.into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Munchiko Calendar is running at http://{address} (Ctrl+C to stop)");

    axum::serve(listener, router(AppState::new(db)))
        .with_graceful_shutdown(async {
            // Finish in-flight requests before exiting.
            let _ = tokio::signal::ctrl_c().await;
            println!("\nShutting down");
        })
        .await?;
    Ok(())
}
