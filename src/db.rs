//! SQLite storage (Milestone 2).
//!
//! [`Repository`] is the only part of the app that knows SQL. It turns rows
//! into the types from [`crate::models`] and back, so the planner (and later
//! the web layer) never sees a database row. The schema itself, with comments
//! on every table, is in `migrations/0001_initial_schema.sql`.
//!
//! Design notes:
//! - Queries are plain strings checked when they run, not sqlx's compile-time
//!   `query!` macros. The macros need a live database (or a prepared cache) on
//!   every build; for an app this size, the unit tests below catch the same
//!   mistakes without that setup.
//! - Enums are stored as lowercase text. The mapping lives here rather than in
//!   `models`, so the domain types stay free of storage details.
//! - Loading a list avoids the "N+1 queries" problem: instead of one extra
//!   query per accessory to fetch its fabrics, we fetch all the accessories,
//!   then all their fabric rows in a single query, and group them in Rust.
//! - Anything that writes several rows that must agree (an accessory and its
//!   fabrics, both halves of a pairing) runs in a transaction: either all of it
//!   is saved or none of it is.
//! - Creating things takes a `New...` struct, which is the model without an
//!   `id` (SQLite assigns that) and without pairings (you pair two things that
//!   already exist, with `pair_fabrics` / `pair_accessories`).

use std::collections::HashMap;
use std::fmt;
use std::ops::RangeInclusive;
use std::str::FromStr;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
};
use sqlx::types::Json;

use crate::holidays::HolidayRule;
use crate::models::*;

/// What `Repository::sync_holidays` changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HolidaySync {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// No longer in the file, but kept because a post is planned for it.
    pub kept_for_plans: usize,
}

/// The migrations in `migrations/`, embedded into the binary at compile time,
/// so the app can bring any database file up to date when it starts.
static MIGRATOR: Migrator = sqlx::migrate!();

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DbError {
    /// There's no row with that id.
    NotFound { what: &'static str, id: i64 },
    /// The row can't be deleted because something else still uses it
    /// (a fabric used by a bow, an accessory in a planned look...).
    InUse { what: &'static str, id: i64 },
    /// A stored value doesn't fit the model, such as an unknown enum label.
    /// The schema's CHECK rules should make this impossible.
    Corrupt(String),
    /// The row comes from `holidays.toml` and can only be changed there.
    ReadOnly { what: &'static str, id: i64 },
    /// Anything else from SQLite, including a CHECK rule rejecting an insert.
    Sqlx(sqlx::Error),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::NotFound { what, id } => write!(f, "there's no {what} #{id}"),
            DbError::InUse { what, id } => {
                write!(f, "{what} #{id} is still in use, so it can't be deleted")
            }
            DbError::ReadOnly { what, id } => {
                write!(f, "{what} #{id} comes from holidays.toml; edit it there")
            }
            DbError::Corrupt(msg) => write!(f, "unexpected data in the database: {msg}"),
            DbError::Sqlx(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DbError::Sqlx(e) => Some(e),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        DbError::Sqlx(e)
    }
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Turn a foreign-key failure from a DELETE into a friendly `InUse` error.
fn in_use(err: sqlx::Error, what: &'static str, id: i64) -> DbError {
    match err.as_database_error() {
        Some(db) if db.is_foreign_key_violation() => DbError::InUse { what, id },
        _ => DbError::Sqlx(err),
    }
}

// ---------------------------------------------------------------------------
// Enums <-> text
// ---------------------------------------------------------------------------

/// An enum stored in a TEXT column.
trait SqlText: Sized {
    fn to_sql(self) -> &'static str;
    fn from_sql(text: &str) -> Result<Self>;
}

/// Writes both directions of the mapping from one table, so they can't drift
/// apart. The labels must match the CHECK (... IN (...)) lists in the schema.
macro_rules! sql_text {
    ($ty:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl SqlText for $ty {
            fn to_sql(self) -> &'static str {
                match self {
                    $($ty::$variant => $text,)+
                }
            }

            fn from_sql(text: &str) -> Result<Self> {
                match text {
                    $($text => Ok($ty::$variant),)+
                    other => Err(DbError::Corrupt(format!(
                        "unknown {} {other:?}",
                        stringify!($ty)
                    ))),
                }
            }
        }
    };
}

sql_text!(Pet { Mochi => "mochi", Momo => "momo" });
sql_text!(AccessoryKind { Bow => "bow", Scarf => "scarf", Costume => "costume" });
sql_text!(FabricRole { Main => "main", Middle => "middle", Accent => "accent" });
sql_text!(AccessoryStatus { Idea => "idea", InProgress => "in_progress", Ready => "ready" });
sql_text!(EventKind {
    Holiday => "holiday",
    NotableDay => "notable_day",
    Seasonal => "seasonal",
    Outing => "outing",
});

// ---------------------------------------------------------------------------
// Inputs for creating things
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewFabric {
    pub name: String,
    pub description: String,
    pub colors: Vec<Color>,
    pub keywords: Vec<String>,
    pub in_stock: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewAccessory {
    pub kind: AccessoryKind,
    pub name: String,
    pub description: String,
    /// Extra colors when `colors_from_fabrics` is on.
    pub colors: Vec<Color>,
    pub colors_from_fabrics: bool,
    pub fabrics: Vec<FabricUse>,
    pub source: Source,
    pub status: AccessoryStatus,
    pub make_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub name: String,
    pub description: String,
    pub kind: EventKind,
    pub start: NaiveDate,
    pub end: Option<NaiveDate>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPostPlan {
    pub event_id: EventId,
    pub post_date: NaiveDate,
    pub looks: Vec<Look>,
    pub backdrop: String,
    pub notes: String,
}

// ---------------------------------------------------------------------------
// Rows: one struct per SELECT, shaped exactly like the table
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct FabricRow {
    id: i64,
    name: String,
    description: String,
    colors: Json<Vec<Color>>,
    keywords: Json<Vec<String>>,
    in_stock: bool,
}

#[derive(FromRow)]
struct AccessoryRow {
    id: i64,
    kind: String,
    name: String,
    description: String,
    colors: Json<Vec<Color>>,
    colors_from_fabrics: bool,
    source: String,
    brand: Option<String>,
    status: String,
    pair_id: Option<i64>,
    make_days: Option<u32>,
}

#[derive(FromRow)]
struct AccessoryFabricRow {
    accessory_id: i64,
    fabric_id: i64,
    role: String,
}

#[derive(FromRow)]
struct EventRow {
    id: i64,
    name: String,
    description: String,
    kind: String,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    keywords: Json<Vec<String>>,
    holiday_key: Option<String>,
}

#[derive(FromRow)]
struct PostPlanRow {
    id: i64,
    event_id: i64,
    post_date: NaiveDate,
    backdrop: String,
    notes: String,
}

#[derive(FromRow)]
struct LookRow {
    post_plan_id: i64,
    pet: String,
    accessory_id: i64,
}

impl FabricRow {
    fn into_model(self, pairings: Vec<FabricId>) -> Fabric {
        Fabric {
            id: FabricId(self.id),
            name: self.name,
            description: self.description,
            colors: self.colors.0,
            keywords: self.keywords.0,
            pairings,
            in_stock: self.in_stock,
        }
    }
}

impl AccessoryRow {
    fn into_model(self, fabrics: Vec<FabricUse>) -> Result<Accessory> {
        let source = match self.source.as_str() {
            "handmade" => Source::Handmade,
            "bought" => Source::Bought { brand: self.brand },
            other => return Err(DbError::Corrupt(format!("unknown Source {other:?}"))),
        };
        Ok(Accessory {
            id: AccessoryId(self.id),
            kind: AccessoryKind::from_sql(&self.kind)?,
            name: self.name,
            description: self.description,
            colors: self.colors.0,
            colors_from_fabrics: self.colors_from_fabrics,
            fabrics,
            source,
            status: AccessoryStatus::from_sql(&self.status)?,
            pair_id: self.pair_id.map(AccessoryId),
            make_days: self.make_days,
        })
    }
}

impl EventRow {
    fn into_model(self) -> Result<Event> {
        Ok(Event {
            id: EventId(self.id),
            name: self.name,
            description: self.description,
            kind: EventKind::from_sql(&self.kind)?,
            start: self.start_date,
            end: self.end_date,
            keywords: self.keywords.0,
            from_holidays_file: self.holiday_key.is_some(),
        })
    }
}

impl PostPlanRow {
    fn into_model(self, looks: Vec<Look>) -> PostPlan {
        PostPlan {
            id: PostPlanId(self.id),
            event_id: EventId(self.event_id),
            post_date: self.post_date,
            looks,
            backdrop: self.backdrop,
            notes: self.notes,
        }
    }
}

/// Split a `Source` into the two columns it's stored in.
fn source_columns(source: &Source) -> (&'static str, Option<&str>) {
    match source {
        Source::Handmade => ("handmade", None),
        Source::Bought { brand } => ("bought", brand.as_deref()),
    }
}

/// Insert an accessory's fabric list. Takes a connection rather than the
/// pool so callers can run it inside their own transaction.
async fn insert_fabric_uses(
    conn: &mut SqliteConnection,
    accessory_id: i64,
    uses: &[FabricUse],
) -> Result<()> {
    for fabric_use in uses {
        sqlx::query(
            "INSERT INTO accessory_fabrics (accessory_id, fabric_id, role)
             VALUES (?, ?, ?)",
        )
        .bind(accessory_id)
        .bind(fabric_use.fabric_id.0)
        .bind(fabric_use.role.to_sql())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Insert a post plan's looks, inside the caller's transaction.
async fn insert_looks(
    conn: &mut SqliteConnection,
    post_plan_id: i64,
    looks: &[Look],
) -> Result<()> {
    for look in looks {
        sqlx::query("INSERT INTO looks (post_plan_id, pet, accessory_id) VALUES (?, ?, ?)")
            .bind(post_plan_id)
            .bind(look.pet.to_sql())
            .bind(look.accessory_id.0)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Take the single row a `get_...` query asked for, or report it missing.
fn exactly_one<T>(mut found: Vec<T>, what: &'static str, id: i64) -> Result<T> {
    found.pop().ok_or(DbError::NotFound { what, id })
}

// ---------------------------------------------------------------------------
// The repository
// ---------------------------------------------------------------------------

/// A handle to the database. It's cheap to clone: clones share one
/// connection pool (a few open connections that requests take turns using).
#[derive(Debug, Clone)]
pub struct Repository {
    pool: SqlitePool,
}

impl Repository {
    /// Open the database at `url` (e.g. `"sqlite:munchiko.db"`), creating the
    /// file if needed, and run any migrations it hasn't seen yet.
    pub async fn connect(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            // SQLite ignores REFERENCES unless this is switched on for each
            // connection. (sqlx does it by default; we say it out loud.)
            .foreign_keys(true)
            // Write-ahead logging: readers don't block the writer, which
            // matters once the web server handles requests side by side.
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        Self::migrate(pool).await
    }

    /// A fresh, empty database that lives only in memory, for tests.
    pub async fn in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        // Every in-memory connection is its own separate database, so the pool
        // must hold exactly one connection and never close it.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        Self::migrate(pool).await
    }

    async fn migrate(pool: SqlitePool) -> Result<Self> {
        MIGRATOR.run(&pool).await.map_err(sqlx::Error::from)?;
        Ok(Self { pool })
    }

    /// True once anything has been entered by hand: a fabric, accessory,
    /// post or your own event. Holidays don't count, since the app adds those
    /// itself. Used to add the example data only to a brand-new database.
    pub async fn has_user_data(&self) -> Result<bool> {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM fabrics)
                 OR EXISTS (SELECT 1 FROM accessories)
                 OR EXISTS (SELECT 1 FROM post_plans)
                 OR EXISTS (SELECT 1 FROM events WHERE holiday_key IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(found)
    }

    // -- Fabrics -------------------------------------------------------------

    pub async fn create_fabric(&self, new: &NewFabric) -> Result<FabricId> {
        // RETURNING hands back the id SQLite just assigned.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO fabrics (name, description, colors, keywords, in_stock)
             VALUES (?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(&new.name)
        .bind(&new.description)
        .bind(Json(&new.colors))
        .bind(Json(&new.keywords))
        .bind(new.in_stock)
        .fetch_one(&self.pool)
        .await?;
        Ok(FabricId(id))
    }

    pub async fn get_fabric(&self, id: FabricId) -> Result<Fabric> {
        exactly_one(self.load_fabrics(Some(id)).await?, "fabric", id.0)
    }

    /// All fabrics, sorted by name.
    pub async fn list_fabrics(&self) -> Result<Vec<Fabric>> {
        self.load_fabrics(None).await
    }

    /// Shared by `get_fabric` (one id) and `list_fabrics` (`None` = all).
    /// `WHERE ?1 IS NULL OR id = ?1` matches every row when no id is given.
    async fn load_fabrics(&self, only: Option<FabricId>) -> Result<Vec<Fabric>> {
        let only = only.map(|id| id.0);
        // A read transaction makes both queries see the same snapshot of the data.
        let mut tx = self.pool.begin().await?;

        let rows: Vec<FabricRow> = sqlx::query_as(
            "SELECT id, name, description, colors, keywords, in_stock
             FROM fabrics
             WHERE ?1 IS NULL OR id = ?1
             ORDER BY name, id",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;

        let pairings: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT fabric_id, paired_fabric_id
             FROM fabric_pairings
             WHERE ?1 IS NULL OR fabric_id = ?1
             ORDER BY fabric_id, paired_fabric_id",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut pairings_by_fabric: HashMap<i64, Vec<FabricId>> = HashMap::new();
        for (fabric_id, paired) in pairings {
            pairings_by_fabric
                .entry(fabric_id)
                .or_default()
                .push(FabricId(paired));
        }

        Ok(rows
            .into_iter()
            .map(|row| {
                let pairings = pairings_by_fabric.remove(&row.id).unwrap_or_default();
                row.into_model(pairings)
            })
            .collect())
    }

    /// Mark fabric as bought (or used up).
    pub async fn set_fabric_in_stock(&self, id: FabricId, in_stock: bool) -> Result<()> {
        let result = sqlx::query("UPDATE fabrics SET in_stock = ? WHERE id = ?")
            .bind(in_stock)
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "fabric",
                id: id.0,
            });
        }
        Ok(())
    }

    /// Record that two fabrics go well together, in both directions.
    /// Pairing them again is harmless.
    pub async fn pair_fabrics(&self, a: FabricId, b: FabricId) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (from, to) in [(a, b), (b, a)] {
            // ON CONFLICT ... DO NOTHING skips the row only if this exact pair
            // already exists; other problems (pairing a fabric with itself, an
            // unknown id) are still reported as errors.
            sqlx::query(
                "INSERT INTO fabric_pairings (fabric_id, paired_fabric_id)
                 VALUES (?, ?)
                 ON CONFLICT (fabric_id, paired_fabric_id) DO NOTHING",
            )
            .bind(from.0)
            .bind(to.0)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Remove a pairing in both directions. A single statement, so it's
    /// already all-or-nothing without a transaction.
    pub async fn unpair_fabrics(&self, a: FabricId, b: FabricId) -> Result<()> {
        sqlx::query(
            "DELETE FROM fabric_pairings
             WHERE (fabric_id = ?1 AND paired_fabric_id = ?2)
                OR (fabric_id = ?2 AND paired_fabric_id = ?1)",
        )
        .bind(a.0)
        .bind(b.0)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Save edits to a fabric's details. Pairings are changed separately
    /// with `set_fabric_pairings`.
    pub async fn update_fabric(&self, id: FabricId, new: &NewFabric) -> Result<()> {
        let result = sqlx::query(
            "UPDATE fabrics
             SET name = ?, description = ?, colors = ?, keywords = ?, in_stock = ?
             WHERE id = ?",
        )
        .bind(&new.name)
        .bind(&new.description)
        .bind(Json(&new.colors))
        .bind(Json(&new.keywords))
        .bind(new.in_stock)
        .bind(id.0)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "fabric",
                id: id.0,
            });
        }
        Ok(())
    }

    /// Make `pairs_with` the complete list of fabrics that go with `id`, in
    /// both directions: fabrics no longer listed lose their link back too.
    /// This is what the edit form's checkboxes need.
    pub async fn set_fabric_pairings(&self, id: FabricId, pairs_with: &[FabricId]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM fabrics WHERE id = ?)")
            .bind(id.0)
            .fetch_one(&mut *tx)
            .await?;
        if !exists {
            return Err(DbError::NotFound {
                what: "fabric",
                id: id.0,
            });
        }

        // Start from nothing, then add each pairing both ways.
        sqlx::query("DELETE FROM fabric_pairings WHERE fabric_id = ?1 OR paired_fabric_id = ?1")
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        for other in pairs_with {
            for (from, to) in [(id, *other), (*other, id)] {
                sqlx::query(
                    "INSERT INTO fabric_pairings (fabric_id, paired_fabric_id)
                     VALUES (?, ?)
                     ON CONFLICT (fabric_id, paired_fabric_id) DO NOTHING",
                )
                .bind(from.0)
                .bind(to.0)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Fails with `InUse` while any accessory is made from this fabric.
    /// Its pairings are removed automatically (ON DELETE CASCADE).
    pub async fn delete_fabric(&self, id: FabricId) -> Result<()> {
        let result = sqlx::query("DELETE FROM fabrics WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| in_use(e, "fabric", id.0))?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "fabric",
                id: id.0,
            });
        }
        Ok(())
    }

    // -- Accessories ---------------------------------------------------------

    /// Saves the accessory and the fabrics it uses together. If any fabric id
    /// doesn't exist, nothing is saved.
    pub async fn create_accessory(&self, new: &NewAccessory) -> Result<AccessoryId> {
        let (source, brand) = source_columns(&new.source);
        let mut tx = self.pool.begin().await?;

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO accessories
                 (kind, name, description, colors, colors_from_fabrics, source, brand, status,
                  make_days)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(new.kind.to_sql())
        .bind(&new.name)
        .bind(&new.description)
        .bind(Json(&new.colors))
        .bind(new.colors_from_fabrics)
        .bind(source)
        .bind(brand)
        .bind(new.status.to_sql())
        .bind(new.make_days)
        .fetch_one(&mut *tx)
        .await?;
        insert_fabric_uses(&mut tx, id, &new.fabrics).await?;

        // If we returned early with an error above, `tx` is dropped without
        // committing and SQLite rolls everything back.
        tx.commit().await?;
        Ok(AccessoryId(id))
    }

    pub async fn get_accessory(&self, id: AccessoryId) -> Result<Accessory> {
        exactly_one(self.load_accessories(Some(id)).await?, "accessory", id.0)
    }

    /// All accessories, sorted by name.
    pub async fn list_accessories(&self) -> Result<Vec<Accessory>> {
        self.load_accessories(None).await
    }

    async fn load_accessories(&self, only: Option<AccessoryId>) -> Result<Vec<Accessory>> {
        let only = only.map(|id| id.0);
        let mut tx = self.pool.begin().await?;

        let rows: Vec<AccessoryRow> = sqlx::query_as(
            "SELECT id, kind, name, description, colors, colors_from_fabrics, source, brand, status,
                    pair_id, make_days
             FROM accessories
             WHERE ?1 IS NULL OR id = ?1
             ORDER BY name, id",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;

        // `rowid` is SQLite's hidden insertion counter, so fabrics come back
        // in the order they were listed when the accessory was created.
        let uses: Vec<AccessoryFabricRow> = sqlx::query_as(
            "SELECT accessory_id, fabric_id, role
             FROM accessory_fabrics
             WHERE ?1 IS NULL OR accessory_id = ?1
             ORDER BY accessory_id, rowid",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut uses_by_accessory: HashMap<i64, Vec<FabricUse>> = HashMap::new();
        for u in uses {
            uses_by_accessory
                .entry(u.accessory_id)
                .or_default()
                .push(FabricUse {
                    fabric_id: FabricId(u.fabric_id),
                    role: FabricRole::from_sql(&u.role)?,
                });
        }

        rows.into_iter()
            .map(|row| {
                let fabrics = uses_by_accessory.remove(&row.id).unwrap_or_default();
                row.into_model(fabrics)
            })
            .collect()
    }

    /// Save edits to an accessory, replacing its list of fabrics. Its matching
    /// partner is left as it was (see `pair_accessories`).
    pub async fn update_accessory(&self, id: AccessoryId, new: &NewAccessory) -> Result<()> {
        let (source, brand) = source_columns(&new.source);
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            "UPDATE accessories
             SET kind = ?, name = ?, description = ?, colors = ?, colors_from_fabrics = ?,
                 source = ?, brand = ?,
                 status = ?, make_days = ?
             WHERE id = ?",
        )
        .bind(new.kind.to_sql())
        .bind(&new.name)
        .bind(&new.description)
        .bind(Json(&new.colors))
        .bind(new.colors_from_fabrics)
        .bind(source)
        .bind(brand)
        .bind(new.status.to_sql())
        .bind(new.make_days)
        .bind(id.0)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "accessory",
                id: id.0,
            });
        }

        sqlx::query("DELETE FROM accessory_fabrics WHERE accessory_id = ?")
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        insert_fabric_uses(&mut tx, id.0, &new.fabrics).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Move an accessory along: idea → in progress → ready.
    pub async fn set_accessory_status(
        &self,
        id: AccessoryId,
        status: AccessoryStatus,
    ) -> Result<()> {
        let result = sqlx::query("UPDATE accessories SET status = ? WHERE id = ?")
            .bind(status.to_sql())
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "accessory",
                id: id.0,
            });
        }
        Ok(())
    }

    /// Make `a` and `b` a matching Mochi & Momo set. Each now points at the
    /// other; if either was already in a set, its old partner is unpaired.
    pub async fn pair_accessories(&self, a: AccessoryId, b: AccessoryId) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for id in [a, b] {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accessories WHERE id = ?)")
                    .bind(id.0)
                    .fetch_one(&mut *tx)
                    .await?;
            if !exists {
                return Err(DbError::NotFound {
                    what: "accessory",
                    id: id.0,
                });
            }
        }

        // Break up any existing sets that involve a or b...
        sqlx::query(
            "UPDATE accessories SET pair_id = NULL
             WHERE id IN (?1, ?2) OR pair_id IN (?1, ?2)",
        )
        .bind(a.0)
        .bind(b.0)
        .execute(&mut *tx)
        .await?;

        // ...then point each one at the other.
        for (from, to) in [(a, b), (b, a)] {
            sqlx::query("UPDATE accessories SET pair_id = ? WHERE id = ?")
                .bind(to.0)
                .bind(from.0)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Take an accessory out of its set; its partner is unpaired too.
    pub async fn unpair_accessory(&self, id: AccessoryId) -> Result<()> {
        sqlx::query("UPDATE accessories SET pair_id = NULL WHERE id = ?1 OR pair_id = ?1")
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fails with `InUse` while a post plan has it in a look. Its fabric list
    /// is removed automatically, and its partner (if any) becomes unpaired.
    pub async fn delete_accessory(&self, id: AccessoryId) -> Result<()> {
        let result = sqlx::query("DELETE FROM accessories WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| in_use(e, "accessory", id.0))?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "accessory",
                id: id.0,
            });
        }
        Ok(())
    }

    // -- Events --------------------------------------------------------------

    pub async fn create_event(&self, new: &NewEvent) -> Result<EventId> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO events (name, description, kind, start_date, end_date, keywords)
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.kind.to_sql())
        .bind(new.start)
        .bind(new.end)
        .bind(Json(&new.keywords))
        .fetch_one(&self.pool)
        .await?;
        Ok(EventId(id))
    }

    pub async fn get_event(&self, id: EventId) -> Result<Event> {
        let row: Option<EventRow> = sqlx::query_as(
            "SELECT id, name, description, kind, start_date, end_date, keywords, holiday_key
             FROM events WHERE id = ?",
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await?;
        row.ok_or(DbError::NotFound {
            what: "event",
            id: id.0,
        })?
        .into_model()
    }

    /// All events, sorted by start date.
    pub async fn list_events(&self) -> Result<Vec<Event>> {
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT id, name, description, kind, start_date, end_date, keywords, holiday_key
             FROM events
             ORDER BY start_date, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(EventRow::into_model).collect()
    }

    /// Every event that touches the range `from..=to`, sorted by start date.
    /// A multi-day event counts if any of its days fall in the range, so Shark
    /// Week shows up on a calendar page even if it started last month.
    pub async fn list_events_between(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Event>> {
        // Two ranges overlap when each one starts before the other ends.
        // COALESCE(end_date, start_date) treats a single-day event as ending
        // the day it starts. ISO dates compare correctly as text.
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT id, name, description, kind, start_date, end_date, keywords, holiday_key
             FROM events
             WHERE start_date <= ?2 AND COALESCE(end_date, start_date) >= ?1
             ORDER BY start_date, id",
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(EventRow::into_model).collect()
    }

    /// The event made from the holidays-file entry `name` for `year`, if any.
    pub async fn find_holiday(&self, name: &str, year: i32) -> Result<Option<Event>> {
        let row: Option<EventRow> = sqlx::query_as(
            "SELECT id, name, description, kind, start_date, end_date, keywords, holiday_key
             FROM events WHERE holiday_key = ? AND holiday_year = ?",
        )
        .bind(name)
        .bind(year)
        .fetch_optional(&self.pool)
        .await?;
        row.map(EventRow::into_model).transpose()
    }

    /// Make the holiday events for `years` match the holiday rules:
    /// - a holiday with no row for that year yet is added;
    /// - an existing row whose name, dates, kind etc. changed is updated in
    ///   place, keeping its id so post plans stay attached;
    /// - a row whose holiday was removed from the file (or has no date that
    ///   year) is deleted, unless a post is planned for it.
    ///
    /// Years outside `years` are left alone, so past holidays stay on the
    /// calendar as history. Running it twice in a row changes nothing.
    pub async fn sync_holidays(
        &self,
        rules: &[HolidayRule],
        years: RangeInclusive<i32>,
    ) -> Result<HolidaySync> {
        let mut report = HolidaySync::default();
        // One transaction: the calendar is never seen half-synced.
        let mut tx = self.pool.begin().await?;

        let existing: Vec<(i64, String, i32)> = sqlx::query_as(
            "SELECT id, holiday_key, holiday_year FROM events
             WHERE holiday_key IS NOT NULL AND holiday_year BETWEEN ? AND ?",
        )
        .bind(years.start())
        .bind(years.end())
        .fetch_all(&mut *tx)
        .await?;
        // (name, year) -> event id. Entries still left at the end are stale.
        let mut existing: HashMap<(String, i32), i64> = existing
            .into_iter()
            .map(|(id, key, year)| ((key, year), id))
            .collect();

        for year in years.clone() {
            for rule in rules {
                let Some(when) = rule.occurrence(year) else {
                    continue;
                };
                match existing.remove(&(rule.name.clone(), year)) {
                    Some(id) => {
                        // The second condition compares all six columns at
                        // once ("row values"), so rows that already match
                        // aren't rewritten or counted as updated.
                        let result = sqlx::query(
                            "UPDATE events
                             SET name = ?1, description = ?2, kind = ?3,
                                 start_date = ?4, end_date = ?5, keywords = ?6
                             WHERE id = ?7
                               AND (name, description, kind, start_date, end_date, keywords)
                                   IS NOT (?1, ?2, ?3, ?4, ?5, ?6)",
                        )
                        .bind(&rule.name)
                        .bind(&rule.description)
                        .bind(rule.kind.to_sql())
                        .bind(when.start)
                        .bind(when.end)
                        .bind(Json(&rule.keywords))
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                        report.updated += result.rows_affected() as usize;
                    }
                    None => {
                        sqlx::query(
                            "INSERT INTO events (name, description, kind, start_date, end_date,
                                                 keywords, holiday_key, holiday_year)
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                        )
                        .bind(&rule.name)
                        .bind(&rule.description)
                        .bind(rule.kind.to_sql())
                        .bind(when.start)
                        .bind(when.end)
                        .bind(Json(&rule.keywords))
                        .bind(&rule.name)
                        .bind(year)
                        .execute(&mut *tx)
                        .await?;
                        report.added += 1;
                    }
                }
            }
        }

        // Whatever is left no longer comes from the file.
        for id in existing.into_values() {
            let result = sqlx::query(
                "DELETE FROM events
                 WHERE id = ?1
                   AND NOT EXISTS (SELECT 1 FROM post_plans WHERE event_id = ?1)",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 1 {
                report.removed += 1;
            } else {
                report.kept_for_plans += 1;
            }
        }

        tx.commit().await?;
        Ok(report)
    }

    /// Save edits to an event you entered yourself. Holiday events fail with
    /// `ReadOnly`: the next sync would overwrite any change anyway.
    pub async fn update_event(&self, id: EventId, new: &NewEvent) -> Result<()> {
        let result = sqlx::query(
            "UPDATE events
             SET name = ?, description = ?, kind = ?, start_date = ?, end_date = ?, keywords = ?
             WHERE id = ? AND holiday_key IS NULL",
        )
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.kind.to_sql())
        .bind(new.start)
        .bind(new.end)
        .bind(Json(&new.keywords))
        .bind(id.0)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(self.why_event_unchanged(id).await);
        }
        Ok(())
    }

    /// Fails with `InUse` while a post is planned for this event, and with
    /// `ReadOnly` for holiday events (the next sync would just add it back).
    pub async fn delete_event(&self, id: EventId) -> Result<()> {
        let result = sqlx::query("DELETE FROM events WHERE id = ? AND holiday_key IS NULL")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| in_use(e, "event", id.0))?;
        if result.rows_affected() == 0 {
            return Err(self.why_event_unchanged(id).await);
        }
        Ok(())
    }

    /// After an UPDATE/DELETE matched no row: was it missing or a holiday?
    async fn why_event_unchanged(&self, id: EventId) -> DbError {
        match self.get_event(id).await {
            Ok(_) => DbError::ReadOnly {
                what: "event",
                id: id.0,
            },
            Err(e) => e,
        }
    }

    // -- Post plans ----------------------------------------------------------

    /// Saves the plan and its looks together.
    pub async fn create_post_plan(&self, new: &NewPostPlan) -> Result<PostPlanId> {
        let mut tx = self.pool.begin().await?;

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO post_plans (event_id, post_date, backdrop, notes)
             VALUES (?, ?, ?, ?)
             RETURNING id",
        )
        .bind(new.event_id.0)
        .bind(new.post_date)
        .bind(&new.backdrop)
        .bind(&new.notes)
        .fetch_one(&mut *tx)
        .await?;

        insert_looks(&mut tx, id, &new.looks).await?;

        tx.commit().await?;
        Ok(PostPlanId(id))
    }

    pub async fn get_post_plan(&self, id: PostPlanId) -> Result<PostPlan> {
        exactly_one(self.load_post_plans(Some(id)).await?, "post plan", id.0)
    }

    /// All post plans, soonest first.
    pub async fn list_post_plans(&self) -> Result<Vec<PostPlan>> {
        self.load_post_plans(None).await
    }

    async fn load_post_plans(&self, only: Option<PostPlanId>) -> Result<Vec<PostPlan>> {
        let only = only.map(|id| id.0);
        let mut tx = self.pool.begin().await?;

        let rows: Vec<PostPlanRow> = sqlx::query_as(
            "SELECT id, event_id, post_date, backdrop, notes
             FROM post_plans
             WHERE ?1 IS NULL OR id = ?1
             ORDER BY post_date, id",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;

        let looks: Vec<LookRow> = sqlx::query_as(
            "SELECT post_plan_id, pet, accessory_id
             FROM looks
             WHERE ?1 IS NULL OR post_plan_id = ?1
             ORDER BY post_plan_id, rowid",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut looks_by_plan: HashMap<i64, Vec<Look>> = HashMap::new();
        for look in looks {
            looks_by_plan
                .entry(look.post_plan_id)
                .or_default()
                .push(Look {
                    pet: Pet::from_sql(&look.pet)?,
                    accessory_id: AccessoryId(look.accessory_id),
                });
        }

        Ok(rows
            .into_iter()
            .map(|row| {
                let looks = looks_by_plan.remove(&row.id).unwrap_or_default();
                row.into_model(looks)
            })
            .collect())
    }

    /// Save edits to a post plan, replacing its looks.
    pub async fn update_post_plan(&self, id: PostPlanId, new: &NewPostPlan) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE post_plans SET event_id = ?, post_date = ?, backdrop = ?, notes = ?
             WHERE id = ?",
        )
        .bind(new.event_id.0)
        .bind(new.post_date)
        .bind(&new.backdrop)
        .bind(&new.notes)
        .bind(id.0)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "post plan",
                id: id.0,
            });
        }

        sqlx::query("DELETE FROM looks WHERE post_plan_id = ?")
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        insert_looks(&mut tx, id.0, &new.looks).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Deletes the plan and its looks (ON DELETE CASCADE).
    pub async fn delete_post_plan(&self, id: PostPlanId) -> Result<()> {
        let result = sqlx::query("DELETE FROM post_plans WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "post plan",
                id: id.0,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests: each one gets its own empty in-memory database.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holidays;
    use crate::planner::{Inventory, LeadTimes, TaskKind, build_timeline};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn colors(list: &[&str]) -> Vec<Color> {
        list.iter().map(|&s| s.into()).collect()
    }

    fn fabric(name: &str, in_stock: bool) -> NewFabric {
        NewFabric {
            name: name.into(),
            description: format!("{name} fabric"),
            colors: colors(&["orange", "black"]),
            keywords: words(&["halloween"]),
            in_stock,
        }
    }

    fn bow(name: &str, fabrics: &[(FabricId, FabricRole)]) -> NewAccessory {
        NewAccessory {
            kind: AccessoryKind::Bow,
            name: name.into(),
            description: String::new(),
            colors: colors(&["orange"]),
            colors_from_fabrics: false,
            fabrics: fabrics
                .iter()
                .map(|&(fabric_id, role)| FabricUse { fabric_id, role })
                .collect(),
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            make_days: None,
        }
    }

    fn event(name: &str, start: NaiveDate, end: Option<NaiveDate>) -> NewEvent {
        NewEvent {
            name: name.into(),
            description: String::new(),
            kind: EventKind::NotableDay,
            start,
            end,
            keywords: words(&["fun"]),
        }
    }

    #[tokio::test]
    async fn fabric_round_trips_including_json_lists() {
        let db = Repository::in_memory().await.unwrap();
        let mut velvet = fabric("Orange velvet", false);
        velvet.colors[0].hex = Some("#f28c28".into());
        let id = db.create_fabric(&velvet).await.unwrap();

        let loaded = db.get_fabric(id).await.unwrap();
        assert_eq!(
            loaded,
            Fabric {
                id,
                name: "Orange velvet".into(),
                description: "Orange velvet fabric".into(),
                // One color with a swatch, one without.
                colors: vec![Color::with_hex("orange", "#f28c28"), "black".into()],
                keywords: words(&["halloween"]),
                pairings: vec![],
                in_stock: false,
            }
        );

        db.set_fabric_in_stock(id, true).await.unwrap();
        assert!(db.get_fabric(id).await.unwrap().in_stock);
        assert!(matches!(
            db.get_fabric(FabricId(99)).await,
            Err(DbError::NotFound {
                what: "fabric",
                id: 99
            })
        ));
    }

    #[tokio::test]
    async fn fabric_pairings_go_both_ways() {
        let db = Repository::in_memory().await.unwrap();
        let orange = db.create_fabric(&fabric("Orange", true)).await.unwrap();
        let black = db.create_fabric(&fabric("Black", true)).await.unwrap();
        let purple = db.create_fabric(&fabric("Purple", true)).await.unwrap();

        db.pair_fabrics(orange, black).await.unwrap();
        db.pair_fabrics(black, orange).await.unwrap(); // already paired: no-op
        db.pair_fabrics(black, purple).await.unwrap();

        assert_eq!(db.get_fabric(orange).await.unwrap().pairings, vec![black]);
        assert_eq!(
            db.get_fabric(black).await.unwrap().pairings,
            vec![orange, purple]
        );

        db.unpair_fabrics(black, orange).await.unwrap();
        assert!(db.get_fabric(orange).await.unwrap().pairings.is_empty());
        assert_eq!(db.get_fabric(black).await.unwrap().pairings, vec![purple]);

        // Deleting a fabric removes its pairings from the other side too.
        db.delete_fabric(purple).await.unwrap();
        assert!(db.get_fabric(black).await.unwrap().pairings.is_empty());

        // The CHECK rule stops a fabric pairing with itself.
        assert!(db.pair_fabrics(black, black).await.is_err());
    }

    #[tokio::test]
    async fn accessory_round_trips_with_fabrics_in_order() {
        let db = Repository::in_memory().await.unwrap();
        let orange = db.create_fabric(&fabric("Orange", true)).await.unwrap();
        let black = db.create_fabric(&fabric("Black", true)).await.unwrap();

        let mut new = bow(
            "Pumpkin bow",
            &[(orange, FabricRole::Main), (black, FabricRole::Middle)],
        );
        new.make_days = Some(5);
        let id = db.create_accessory(&new).await.unwrap();

        let loaded = db.get_accessory(id).await.unwrap();
        assert_eq!(loaded.kind, AccessoryKind::Bow);
        assert_eq!(loaded.fabrics, new.fabrics);
        assert_eq!(loaded.source, Source::Handmade);
        assert_eq!(loaded.make_days, Some(5));
        assert_eq!(loaded.pair_id, None);

        db.set_accessory_status(id, AccessoryStatus::InProgress)
            .await
            .unwrap();
        assert_eq!(
            db.get_accessory(id).await.unwrap().status,
            AccessoryStatus::InProgress
        );
    }

    #[tokio::test]
    async fn bought_accessory_keeps_its_brand() {
        let db = Repository::in_memory().await.unwrap();
        let scarf = NewAccessory {
            kind: AccessoryKind::Scarf,
            source: Source::Bought {
                brand: Some("Spooky Pets Co".into()),
            },
            ..bow("Ghost scarf", &[])
        };
        let id = db.create_accessory(&scarf).await.unwrap();

        let loaded = db.get_accessory(id).await.unwrap();
        assert_eq!(loaded.kind, AccessoryKind::Scarf);
        assert_eq!(loaded.source, scarf.source);
        assert!(loaded.fabrics.is_empty());
    }

    #[tokio::test]
    async fn creating_an_accessory_with_unknown_fabric_saves_nothing() {
        let db = Repository::in_memory().await.unwrap();
        let result = db
            .create_accessory(&bow("Mystery bow", &[(FabricId(42), FabricRole::Main)]))
            .await;

        assert!(result.is_err());
        // The transaction rolled back, so the accessory row isn't there either.
        assert!(db.list_accessories().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn accessory_pairs_are_two_way_and_exclusive() {
        let db = Repository::in_memory().await.unwrap();
        let pumpkin = db.create_accessory(&bow("Pumpkin", &[])).await.unwrap();
        let bat = db.create_accessory(&bow("Bat", &[])).await.unwrap();
        let ghost = db.create_accessory(&bow("Ghost", &[])).await.unwrap();

        db.pair_accessories(pumpkin, bat).await.unwrap();
        assert_eq!(db.get_accessory(pumpkin).await.unwrap().pair_id, Some(bat));
        assert_eq!(db.get_accessory(bat).await.unwrap().pair_id, Some(pumpkin));

        // Re-pairing the pumpkin bow leaves the bat bow on its own.
        db.pair_accessories(pumpkin, ghost).await.unwrap();
        assert_eq!(
            db.get_accessory(pumpkin).await.unwrap().pair_id,
            Some(ghost)
        );
        assert_eq!(
            db.get_accessory(ghost).await.unwrap().pair_id,
            Some(pumpkin)
        );
        assert_eq!(db.get_accessory(bat).await.unwrap().pair_id, None);

        // Deleting one half clears the other half's link (ON DELETE SET NULL).
        db.delete_accessory(ghost).await.unwrap();
        assert_eq!(db.get_accessory(pumpkin).await.unwrap().pair_id, None);

        db.pair_accessories(pumpkin, bat).await.unwrap();
        db.unpair_accessory(bat).await.unwrap();
        assert_eq!(db.get_accessory(pumpkin).await.unwrap().pair_id, None);

        assert!(matches!(
            db.pair_accessories(pumpkin, AccessoryId(99)).await,
            Err(DbError::NotFound {
                what: "accessory",
                id: 99
            })
        ));
        assert!(db.pair_accessories(pumpkin, pumpkin).await.is_err());
    }

    #[tokio::test]
    async fn fabric_used_by_an_accessory_cannot_be_deleted() {
        let db = Repository::in_memory().await.unwrap();
        let velvet = db.create_fabric(&fabric("Velvet", true)).await.unwrap();
        let bow_id = db
            .create_accessory(&bow("Velvet bow", &[(velvet, FabricRole::Main)]))
            .await
            .unwrap();

        assert!(matches!(
            db.delete_fabric(velvet).await,
            Err(DbError::InUse { what: "fabric", .. })
        ));

        // Once the bow is gone, so is its fabric list, and the fabric is free.
        db.delete_accessory(bow_id).await.unwrap();
        db.delete_fabric(velvet).await.unwrap();
        assert!(db.list_fabrics().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn events_in_range_include_overlapping_multi_day_events() {
        let db = Repository::in_memory().await.unwrap();
        // Shark Week starts in July and runs into August.
        let shark_week = db
            .create_event(&event("Shark Week", d(2026, 7, 26), Some(d(2026, 8, 1))))
            .await
            .unwrap();
        let earth_day = db
            .create_event(&event("Earth Day", d(2026, 4, 22), None))
            .await
            .unwrap();
        db.create_event(&event("Ides of March", d(2026, 3, 15), None))
            .await
            .unwrap();

        let august = db
            .list_events_between(d(2026, 8, 1), d(2026, 8, 31))
            .await
            .unwrap();
        assert_eq!(
            august.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![shark_week]
        );
        assert_eq!(august[0].end, Some(d(2026, 8, 1)));

        let spring = db
            .list_events_between(d(2026, 4, 1), d(2026, 4, 30))
            .await
            .unwrap();
        assert_eq!(
            spring.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![earth_day]
        );
    }

    #[tokio::test]
    async fn event_cannot_end_before_it_starts() {
        let db = Repository::in_memory().await.unwrap();
        let backwards = event("Backwards week", d(2026, 5, 10), Some(d(2026, 5, 3)));
        assert!(matches!(
            db.create_event(&backwards).await,
            Err(DbError::Sqlx(_))
        ));
    }

    #[tokio::test]
    async fn post_plan_round_trips_and_protects_what_it_uses() {
        let db = Repository::in_memory().await.unwrap();
        let halloween = db
            .create_event(&event("Halloween", d(2026, 10, 31), None))
            .await
            .unwrap();
        let pumpkin = db.create_accessory(&bow("Pumpkin", &[])).await.unwrap();
        let bat = db.create_accessory(&bow("Bat", &[])).await.unwrap();

        let new = NewPostPlan {
            event_id: halloween,
            post_date: d(2026, 10, 31),
            looks: vec![
                Look {
                    pet: Pet::Mochi,
                    accessory_id: pumpkin,
                },
                Look {
                    pet: Pet::Momo,
                    accessory_id: bat,
                },
            ],
            backdrop: "hay bales".into(),
            notes: String::new(),
        };
        let id = db.create_post_plan(&new).await.unwrap();

        let loaded = db.get_post_plan(id).await.unwrap();
        assert_eq!(loaded.looks, new.looks);
        assert_eq!(loaded.post_date, d(2026, 10, 31));

        // The event and the accessories are in use while the plan exists...
        assert!(matches!(
            db.delete_event(halloween).await,
            Err(DbError::InUse { .. })
        ));
        assert!(matches!(
            db.delete_accessory(bat).await,
            Err(DbError::InUse { .. })
        ));

        // ...and free again once it's deleted (its looks go with it).
        db.delete_post_plan(id).await.unwrap();
        db.delete_accessory(bat).await.unwrap();
        db.delete_event(halloween).await.unwrap();
    }

    /// The whole point of M2: data saved in SQLite feeds the M1 planner.
    #[tokio::test]
    async fn planner_works_on_data_loaded_from_the_database() {
        let db = Repository::in_memory().await.unwrap();
        let orange = db
            .create_fabric(&fabric("Orange velvet", false))
            .await
            .unwrap();
        let pumpkin = db
            .create_accessory(&bow("Pumpkin bow", &[(orange, FabricRole::Main)]))
            .await
            .unwrap();
        let halloween = db
            .create_event(&event("Halloween", d(2026, 10, 31), None))
            .await
            .unwrap();
        let plan_id = db
            .create_post_plan(&NewPostPlan {
                event_id: halloween,
                post_date: d(2026, 10, 31),
                looks: vec![Look {
                    pet: Pet::Mochi,
                    accessory_id: pumpkin,
                }],
                backdrop: "hay bales".into(),
                notes: String::new(),
            })
            .await
            .unwrap();

        let fabrics = db.list_fabrics().await.unwrap();
        let accessories = db.list_accessories().await.unwrap();
        let plan = db.get_post_plan(plan_id).await.unwrap();
        let inventory = Inventory {
            accessories: &accessories,
            fabrics: &fabrics,
        };
        let timeline = build_timeline(&plan, &inventory, &LeadTimes::default()).unwrap();

        // Same dates as the M1 Halloween example: shoot Oct 28, bow made
        // Oct 24-27, and the out-of-stock velvet bought Oct 21-23.
        assert_eq!(timeline.shoot_date, d(2026, 10, 28));
        let buy = timeline
            .tasks
            .iter()
            .find(|t| t.kind == TaskKind::BuyFabric)
            .unwrap();
        assert_eq!((buy.start, buy.due), (d(2026, 10, 21), d(2026, 10, 23)));
    }

    const HOLIDAYS: &str = r#"
        [[holiday]]
        name = "Halloween"
        date = "10-31"
        keywords = ["spooky"]

        [[holiday]]
        name = "Thanksgiving"
        nth_weekday = { n = 4, weekday = "thu", month = 11 }

        [[holiday]]
        name = "Shark Week"
        kind = "notable_day"
        days = 7
        by_year = { 2027 = "07-11" }
    "#;

    #[tokio::test]
    async fn syncing_holidays_adds_each_year_once() {
        let db = Repository::in_memory().await.unwrap();
        let rules = holidays::parse(HOLIDAYS).unwrap();

        let first = db.sync_holidays(&rules, 2026..=2027).await.unwrap();
        // Halloween and Thanksgiving both years; Shark Week only has a 2027 date.
        assert_eq!(
            first,
            HolidaySync {
                added: 5,
                ..Default::default()
            }
        );

        let again = db.sync_holidays(&rules, 2026..=2027).await.unwrap();
        assert_eq!(
            again,
            HolidaySync::default(),
            "a second sync changes nothing"
        );

        let thanksgiving = db
            .find_holiday("Thanksgiving", 2027)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(thanksgiving.start, d(2027, 11, 25));
        let shark_week = db.find_holiday("Shark Week", 2027).await.unwrap().unwrap();
        assert_eq!(shark_week.kind, EventKind::NotableDay);
        assert_eq!(shark_week.end, Some(d(2027, 7, 17)));
        assert!(db.find_holiday("Shark Week", 2026).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn editing_the_file_updates_rows_in_place() {
        let db = Repository::in_memory().await.unwrap();
        db.sync_holidays(&holidays::parse(HOLIDAYS).unwrap(), 2027..=2027)
            .await
            .unwrap();
        let before = db.find_holiday("Shark Week", 2027).await.unwrap().unwrap();

        // The 2027 date was wrong; fix it in the "file".
        let fixed = HOLIDAYS.replace("07-11", "07-25");
        let report = db
            .sync_holidays(&holidays::parse(&fixed).unwrap(), 2027..=2027)
            .await
            .unwrap();
        assert_eq!(
            report,
            HolidaySync {
                updated: 1,
                ..Default::default()
            }
        );

        let after = db.find_holiday("Shark Week", 2027).await.unwrap().unwrap();
        assert_eq!(after.id, before.id, "same row, so post plans stay attached");
        assert_eq!(
            (after.start, after.end),
            (d(2027, 7, 25), Some(d(2027, 7, 31)))
        );
    }

    #[tokio::test]
    async fn removed_holidays_go_unless_a_post_is_planned() {
        let db = Repository::in_memory().await.unwrap();
        db.sync_holidays(&holidays::parse(HOLIDAYS).unwrap(), 2026..=2027)
            .await
            .unwrap();
        let outing = db
            .create_event(&event("Beach trip", d(2026, 11, 7), None))
            .await
            .unwrap();
        let thanksgiving_2026 = db
            .find_holiday("Thanksgiving", 2026)
            .await
            .unwrap()
            .unwrap();
        db.create_post_plan(&NewPostPlan {
            event_id: thanksgiving_2026.id,
            post_date: d(2026, 11, 26),
            looks: vec![],
            backdrop: String::new(),
            notes: String::new(),
        })
        .await
        .unwrap();

        // Only Halloween is left in the file.
        let only_halloween =
            "[[holiday]]\nname = \"Halloween\"\ndate = \"10-31\"\nkeywords = [\"spooky\"]";
        let report = db
            .sync_holidays(&holidays::parse(only_halloween).unwrap(), 2026..=2027)
            .await
            .unwrap();
        assert_eq!(
            report,
            HolidaySync {
                removed: 2,
                kept_for_plans: 1,
                ..Default::default()
            }
        );
        assert!(
            db.find_holiday("Thanksgiving", 2026)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            db.find_holiday("Thanksgiving", 2027)
                .await
                .unwrap()
                .is_none()
        );
        assert!(db.find_holiday("Shark Week", 2027).await.unwrap().is_none());
        // Events you entered yourself are never touched.
        assert_eq!(db.get_event(outing).await.unwrap().name, "Beach trip");
    }

    #[tokio::test]
    async fn sync_leaves_years_outside_the_range_alone() {
        let db = Repository::in_memory().await.unwrap();
        let rules = holidays::parse(HOLIDAYS).unwrap();
        db.sync_holidays(&rules, 2026..=2026).await.unwrap();

        // A year later the app syncs 2027-2028; 2026 stays as history.
        db.sync_holidays(&rules, 2027..=2028).await.unwrap();
        assert!(db.find_holiday("Halloween", 2026).await.unwrap().is_some());
        assert!(db.find_holiday("Halloween", 2028).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn migration_0004_turns_color_words_into_objects() {
        // A fresh test database has no old rows for the migration to convert,
        // so write one the old way (plain strings) and run its SQL again.
        let db = Repository::in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO fabrics (name, description, colors, keywords, in_stock)
             VALUES ('Old', '', '[\"baby pink\",\"white\"]', '[]', 1),
                    ('Plain', '', '[]', '[]', 1)",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../migrations/0004_color_swatches.sql"))
            .execute(&db.pool)
            .await
            .unwrap();

        let fabrics = db.list_fabrics().await.unwrap();
        let old = fabrics.iter().find(|f| f.name == "Old").unwrap();
        assert_eq!(old.colors, colors(&["baby pink", "white"]), "order kept");
        let plain = fabrics.iter().find(|f| f.name == "Plain").unwrap();
        assert!(plain.colors.is_empty(), "empty stays empty");
    }

    #[tokio::test]
    async fn updating_a_fabric_keeps_its_pairings() {
        let db = Repository::in_memory().await.unwrap();
        let orange = db.create_fabric(&fabric("Orange", false)).await.unwrap();
        let black = db.create_fabric(&fabric("Black", true)).await.unwrap();
        db.pair_fabrics(orange, black).await.unwrap();

        let mut edited = fabric("Burnt orange", true);
        edited.colors = colors(&["orange", "brown"]);
        db.update_fabric(orange, &edited).await.unwrap();

        let loaded = db.get_fabric(orange).await.unwrap();
        assert_eq!(loaded.name, "Burnt orange");
        assert_eq!(loaded.colors, colors(&["orange", "brown"]));
        assert!(loaded.in_stock);
        assert_eq!(loaded.pairings, vec![black]);

        assert!(matches!(
            db.update_fabric(FabricId(99), &edited).await,
            Err(DbError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn setting_pairings_replaces_them_in_both_directions() {
        let db = Repository::in_memory().await.unwrap();
        let a = db.create_fabric(&fabric("A", true)).await.unwrap();
        let b = db.create_fabric(&fabric("B", true)).await.unwrap();
        let c = db.create_fabric(&fabric("C", true)).await.unwrap();
        db.pair_fabrics(a, b).await.unwrap();

        // The edit form for A now has only C ticked.
        db.set_fabric_pairings(a, &[c]).await.unwrap();
        assert_eq!(db.get_fabric(a).await.unwrap().pairings, vec![c]);
        assert!(
            db.get_fabric(b).await.unwrap().pairings.is_empty(),
            "B lost its link back"
        );
        assert_eq!(db.get_fabric(c).await.unwrap().pairings, vec![a]);

        db.set_fabric_pairings(a, &[]).await.unwrap();
        assert!(db.get_fabric(c).await.unwrap().pairings.is_empty());
        assert!(matches!(
            db.set_fabric_pairings(FabricId(99), &[a]).await,
            Err(DbError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn updating_an_accessory_replaces_fabrics_and_keeps_its_partner() {
        let db = Repository::in_memory().await.unwrap();
        let velvet = db.create_fabric(&fabric("Velvet", true)).await.unwrap();
        let satin = db.create_fabric(&fabric("Satin", true)).await.unwrap();
        let pumpkin = db
            .create_accessory(&bow("Pumpkin", &[(velvet, FabricRole::Main)]))
            .await
            .unwrap();
        let bat = db.create_accessory(&bow("Bat", &[])).await.unwrap();
        db.pair_accessories(pumpkin, bat).await.unwrap();

        let mut edited = bow(
            "Pumpkin deluxe",
            &[(satin, FabricRole::Main), (velvet, FabricRole::Accent)],
        );
        edited.source = Source::Bought {
            brand: Some("Etsy".into()),
        };
        edited.make_days = Some(6);
        db.update_accessory(pumpkin, &edited).await.unwrap();

        let loaded = db.get_accessory(pumpkin).await.unwrap();
        assert_eq!(loaded.name, "Pumpkin deluxe");
        assert_eq!(loaded.fabrics, edited.fabrics);
        assert_eq!(loaded.source, edited.source);
        assert_eq!(loaded.make_days, Some(6));
        assert_eq!(loaded.pair_id, Some(bat));

        // A bad fabric id rolls the whole edit back.
        let broken = bow("Broken", &[(FabricId(99), FabricRole::Main)]);
        assert!(db.update_accessory(pumpkin, &broken).await.is_err());
        assert_eq!(
            db.get_accessory(pumpkin).await.unwrap().name,
            "Pumpkin deluxe"
        );
    }

    #[tokio::test]
    async fn your_own_events_can_be_edited_but_holidays_cannot() {
        let db = Repository::in_memory().await.unwrap();
        let trip = db
            .create_event(&event("Beach trip", d(2026, 7, 4), None))
            .await
            .unwrap();
        assert!(!db.get_event(trip).await.unwrap().from_holidays_file);

        let moved = event("Beach weekend", d(2026, 7, 11), Some(d(2026, 7, 12)));
        db.update_event(trip, &moved).await.unwrap();
        let loaded = db.get_event(trip).await.unwrap();
        assert_eq!(
            (loaded.name.as_str(), loaded.end),
            ("Beach weekend", Some(d(2026, 7, 12)))
        );

        db.sync_holidays(&holidays::parse(HOLIDAYS).unwrap(), 2026..=2026)
            .await
            .unwrap();
        let halloween = db.find_holiday("Halloween", 2026).await.unwrap().unwrap();
        assert!(halloween.from_holidays_file);
        assert!(matches!(
            db.update_event(halloween.id, &moved).await,
            Err(DbError::ReadOnly { .. })
        ));
        assert!(matches!(
            db.delete_event(halloween.id).await,
            Err(DbError::ReadOnly { .. })
        ));
        assert!(matches!(
            db.update_event(EventId(999), &moved).await,
            Err(DbError::NotFound { .. })
        ));

        db.delete_event(trip).await.unwrap();
    }

    #[tokio::test]
    async fn updating_a_post_plan_replaces_its_looks() {
        let db = Repository::in_memory().await.unwrap();
        let halloween = db
            .create_event(&event("Halloween", d(2026, 10, 31), None))
            .await
            .unwrap();
        let other = db
            .create_event(&event("Black Cat Day", d(2026, 10, 27), None))
            .await
            .unwrap();
        let pumpkin = db.create_accessory(&bow("Pumpkin", &[])).await.unwrap();
        let bat = db.create_accessory(&bow("Bat", &[])).await.unwrap();
        let plan = db
            .create_post_plan(&NewPostPlan {
                event_id: halloween,
                post_date: d(2026, 10, 31),
                looks: vec![Look {
                    pet: Pet::Mochi,
                    accessory_id: pumpkin,
                }],
                backdrop: String::new(),
                notes: String::new(),
            })
            .await
            .unwrap();

        let edited = NewPostPlan {
            event_id: other,
            post_date: d(2026, 10, 27),
            looks: vec![Look {
                pet: Pet::Momo,
                accessory_id: bat,
            }],
            backdrop: "black velvet".into(),
            notes: "Momo solo".into(),
        };
        db.update_post_plan(plan, &edited).await.unwrap();
        let loaded = db.get_post_plan(plan).await.unwrap();
        assert_eq!(loaded.event_id, other);
        assert_eq!(loaded.looks, edited.looks);
        assert_eq!(loaded.notes, "Momo solo");

        // The pumpkin bow is no longer in a look, so it can be deleted now.
        db.delete_accessory(pumpkin).await.unwrap();
    }

    #[tokio::test]
    async fn user_data_means_anything_but_holidays() {
        let db = Repository::in_memory().await.unwrap();
        db.sync_holidays(&holidays::parse(HOLIDAYS).unwrap(), 2026..=2026)
            .await
            .unwrap();
        assert!(
            !db.has_user_data().await.unwrap(),
            "holidays alone don't count"
        );

        // A single fabric, with no posts at all, is enough.
        let velvet = db.create_fabric(&fabric("Velvet", true)).await.unwrap();
        assert!(db.has_user_data().await.unwrap());
        db.delete_fabric(velvet).await.unwrap();

        db.create_event(&event("Beach trip", d(2026, 7, 4), None))
            .await
            .unwrap();
        assert!(db.has_user_data().await.unwrap());
    }
}
