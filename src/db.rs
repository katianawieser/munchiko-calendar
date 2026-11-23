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
use crate::photos::ImageType;

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
    /// Another row already has this name (palette colors are unique by name).
    Duplicate { what: &'static str, name: String },
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
            DbError::Duplicate { what, name } => {
                write!(f, "there's already a {what} called {name:?}")
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
fn duplicate(err: sqlx::Error, what: &'static str, name: &str) -> DbError {
    match err.as_database_error() {
        Some(db) if db.is_unique_violation() => DbError::Duplicate {
            what,
            name: name.to_string(),
        },
        _ => DbError::Sqlx(err),
    }
}

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
sql_text!(AccessoryKind {
    Bow => "bow",
    Scarf => "scarf",
    Costume => "costume",
    Hat => "hat",
    Prop => "prop",
});
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
    pub tags: Vec<String>,
    pub fabrics: Vec<FabricUse>,
    pub source: Source,
    pub status: AccessoryStatus,
    pub make_days: Option<u32>,
    /// How many there are (or will be made). At least 1; the table checks.
    pub quantity: u32,
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
    pub title: String,
    pub event_id: Option<EventId>,
    /// `None` = an idea, not scheduled yet.
    pub post_date: Option<NaiveDate>,
    pub series_id: Option<SeriesId>,
    pub looks: Vec<Look>,
    /// Outfits chosen per pet (their pieces are already in `looks`).
    pub outfits: Vec<OutfitChoice>,
    pub backdrop: String,
    pub notes: String,
}

impl From<PostPlan> for NewPostPlan {
    /// A saved plan as the fields to save it again with, for small changes
    /// like scheduling an idea.
    fn from(plan: PostPlan) -> Self {
        NewPostPlan {
            title: plan.title,
            event_id: plan.event_id,
            post_date: plan.post_date,
            series_id: plan.series_id,
            looks: plan.looks,
            outfits: plan.outfits,
            backdrop: plan.backdrop,
            notes: plan.notes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewOutfit {
    pub name: String,
    pub description: String,
    /// In order; each must be an existing accessory.
    pub pieces: Vec<AccessoryId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSeries {
    pub title: String,
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
    keywords: Json<Vec<String>>,
    in_stock: bool,
}

#[derive(FromRow)]
struct AccessoryRow {
    id: i64,
    kind: String,
    name: String,
    description: String,
    colors_from_fabrics: bool,
    tags: Json<Vec<String>>,
    source: String,
    brand: Option<String>,
    status: String,
    pair_id: Option<i64>,
    make_days: Option<u32>,
    quantity: u32,
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
    title: String,
    event_id: Option<i64>,
    post_date: Option<NaiveDate>,
    series_id: Option<i64>,
    backdrop: String,
    notes: String,
}

#[derive(FromRow)]
struct SeriesRow {
    id: i64,
    title: String,
    backdrop: String,
    notes: String,
}

#[derive(FromRow)]
struct PhotoRow {
    id: i64,
    fabric_id: Option<i64>,
    accessory_id: Option<i64>,
    post_plan_id: Option<i64>,
    outfit_id: Option<i64>,
    content_type: String,
}

#[derive(FromRow)]
struct OutfitRow {
    id: i64,
    name: String,
    description: String,
}

impl SeriesRow {
    fn into_model(self) -> Series {
        Series {
            id: SeriesId(self.id),
            title: self.title,
            backdrop: self.backdrop,
            notes: self.notes,
        }
    }
}

impl PhotoRow {
    fn into_model(self) -> Result<Photo> {
        let owner = match (
            self.fabric_id,
            self.accessory_id,
            self.post_plan_id,
            self.outfit_id,
        ) {
            (Some(id), None, None, None) => PhotoOwner::Fabric(FabricId(id)),
            (None, Some(id), None, None) => PhotoOwner::Accessory(AccessoryId(id)),
            (None, None, Some(id), None) => PhotoOwner::PostPlan(PostPlanId(id)),
            (None, None, None, Some(id)) => PhotoOwner::Outfit(OutfitId(id)),
            _ => {
                return Err(DbError::Corrupt(format!(
                    "photo {} has no single owner",
                    self.id
                )));
            }
        };
        let image_type = ImageType::from_mime(&self.content_type).ok_or_else(|| {
            DbError::Corrupt(format!("unknown photo type {:?}", self.content_type))
        })?;
        Ok(Photo {
            id: PhotoId(self.id),
            owner,
            image_type,
        })
    }
}

#[derive(FromRow)]
struct LookRow {
    post_plan_id: i64,
    pet: String,
    accessory_id: i64,
}

impl FabricRow {
    fn into_model(self, pairings: Vec<FabricId>, colors: Vec<Color>) -> Fabric {
        Fabric {
            id: FabricId(self.id),
            name: self.name,
            description: self.description,
            colors,
            keywords: self.keywords.0,
            pairings,
            in_stock: self.in_stock,
        }
    }
}

impl AccessoryRow {
    fn into_model(self, fabrics: Vec<FabricUse>, colors: Vec<Color>) -> Result<Accessory> {
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
            colors,
            colors_from_fabrics: self.colors_from_fabrics,
            tags: self.tags.0,
            fabrics,
            source,
            status: AccessoryStatus::from_sql(&self.status)?,
            pair_id: self.pair_id.map(AccessoryId),
            make_days: self.make_days,
            quantity: self.quantity,
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
    fn into_model(self, looks: Vec<Look>, outfits: Vec<OutfitChoice>) -> PostPlan {
        PostPlan {
            id: PostPlanId(self.id),
            title: self.title,
            event_id: self.event_id.map(EventId),
            post_date: self.post_date,
            series_id: self.series_id.map(SeriesId),
            looks,
            outfits,
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

/// Which list of item colors: fabrics' or accessories'.
#[derive(Clone, Copy)]
enum ColorList {
    Fabric,
    Accessory,
}

impl ColorList {
    /// The link table and its owner column.
    fn table(self) -> (&'static str, &'static str) {
        match self {
            ColorList::Fabric => ("fabric_colors", "fabric_id"),
            ColorList::Accessory => ("accessory_colors", "accessory_id"),
        }
    }
}

/// The palette colors for `colors`, by name (ignoring capitals and spaces
/// around it). A name that isn't in the palette yet is added with its shade.
/// The palette's shade wins over the one given, except that a palette color
/// without a shade takes it. The same palette color twice counts once.
async fn palette_ids_for(conn: &mut SqliteConnection, colors: &[Color]) -> Result<Vec<i64>> {
    let mut ids: Vec<i64> = Vec::new();
    for color in colors {
        let name = color.name.trim();
        if name.is_empty() {
            continue;
        }
        let found: Option<(i64, Option<String>)> =
            sqlx::query_as("SELECT id, hex FROM palette_colors WHERE name = ? COLLATE NOCASE")
                .bind(name)
                .fetch_optional(&mut *conn)
                .await?;
        let id = match found {
            Some((id, None)) if color.hex.is_some() => {
                sqlx::query("UPDATE palette_colors SET hex = ? WHERE id = ?")
                    .bind(&color.hex)
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
                id
            }
            Some((id, _)) => id,
            None => {
                sqlx::query_scalar(
                    "INSERT INTO palette_colors (name, hex) VALUES (?, ?) RETURNING id",
                )
                .bind(name)
                .bind(&color.hex)
                .fetch_one(&mut *conn)
                .await?
            }
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Replace an item's colors with `colors` (see [`palette_ids_for`]), inside
/// the caller's transaction.
async fn set_item_colors(
    conn: &mut SqliteConnection,
    list: ColorList,
    owner: i64,
    colors: &[Color],
) -> Result<()> {
    let ids = palette_ids_for(conn, colors).await?;
    let (table, column) = list.table();
    sqlx::query(&format!("DELETE FROM {table} WHERE {column} = ?"))
        .bind(owner)
        .execute(&mut *conn)
        .await?;
    for (position, id) in ids.iter().enumerate() {
        sqlx::query(&format!(
            "INSERT INTO {table} ({column}, position, palette_color_id) VALUES (?, ?, ?)"
        ))
        .bind(owner)
        .bind(position as i64)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Items' colors (one item, or all for `None`), in order, by item id.
async fn load_item_colors(
    conn: &mut SqliteConnection,
    list: ColorList,
    only: Option<i64>,
) -> Result<HashMap<i64, Vec<Color>>> {
    let (table, column) = list.table();
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(&format!(
        "SELECT l.{column}, p.name, p.hex
         FROM {table} l JOIN palette_colors p ON p.id = l.palette_color_id
         WHERE ?1 IS NULL OR l.{column} = ?1
         ORDER BY l.{column}, l.position"
    ))
    .bind(only)
    .fetch_all(&mut *conn)
    .await?;
    let mut by_owner: HashMap<i64, Vec<Color>> = HashMap::new();
    for (owner, name, hex) in rows {
        by_owner.entry(owner).or_default().push(Color { name, hex });
    }
    Ok(by_owner)
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

/// Insert a post plan's outfit choices and what each knew, inside the
/// caller's transaction.
async fn insert_outfit_choices(
    conn: &mut SqliteConnection,
    post_plan_id: i64,
    choices: &[OutfitChoice],
) -> Result<()> {
    for choice in choices {
        sqlx::query(
            "INSERT OR IGNORE INTO post_outfits (post_plan_id, pet, outfit_id) VALUES (?, ?, ?)",
        )
        .bind(post_plan_id)
        .bind(choice.pet.to_sql())
        .bind(choice.outfit_id.0)
        .execute(&mut *conn)
        .await?;
        for piece in &choice.known {
            sqlx::query(
                "INSERT OR IGNORE INTO post_outfit_known (post_plan_id, pet, outfit_id, accessory_id)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(post_plan_id)
            .bind(choice.pet.to_sql())
            .bind(choice.outfit_id.0)
            .bind(piece.0)
            .execute(&mut *conn)
            .await?;
        }
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

    /// Saves the fabric and its colors (see [`palette_ids_for`]) together.
    pub async fn create_fabric(&self, new: &NewFabric) -> Result<FabricId> {
        let mut tx = self.pool.begin().await?;
        // RETURNING hands back the id SQLite just assigned.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO fabrics (name, description, keywords, in_stock)
             VALUES (?, ?, ?, ?)
             RETURNING id",
        )
        .bind(&new.name)
        .bind(&new.description)
        .bind(Json(&new.keywords))
        .bind(new.in_stock)
        .fetch_one(&mut *tx)
        .await?;
        set_item_colors(&mut tx, ColorList::Fabric, id, &new.colors).await?;
        tx.commit().await?;
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
            "SELECT id, name, description, keywords, in_stock
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
        let mut colors = load_item_colors(&mut tx, ColorList::Fabric, only).await?;
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
                let colors = colors.remove(&row.id).unwrap_or_default();
                row.into_model(pairings, colors)
            })
            .collect())
    }

    // -- Palette -------------------------------------------------------------

    /// Every palette color, by name.
    pub async fn list_palette(&self) -> Result<Vec<PaletteColor>> {
        let rows: Vec<(i64, String, Option<String>, bool)> = sqlx::query_as(
            "SELECT id, name, hex, name_kept FROM palette_colors ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, hex, name_kept)| PaletteColor {
                id: PaletteColorId(id),
                name,
                hex,
                name_kept,
            })
            .collect())
    }

    /// Keep these palette colors' names (`ids`), or with `None` every
    /// color's: the closest-name hints stop showing for them.
    pub async fn keep_palette_names(&self, ids: Option<&[PaletteColorId]>) -> Result<()> {
        match ids {
            None => {
                sqlx::query("UPDATE palette_colors SET name_kept = 1")
                    .execute(&self.pool)
                    .await?;
            }
            Some(ids) => {
                for id in ids {
                    sqlx::query("UPDATE palette_colors SET name_kept = 1 WHERE id = ?")
                        .bind(id.0)
                        .execute(&self.pool)
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Who uses each palette color: (color, fabric or accessory), one row per
    /// use.
    pub async fn palette_uses(&self) -> Result<Vec<(PaletteColorId, crate::shades::Owner)>> {
        use crate::shades::Owner;
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT palette_color_id, 'fabric', fabric_id FROM fabric_colors
             UNION ALL
             SELECT palette_color_id, 'accessory', accessory_id FROM accessory_colors",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(color, kind, owner)| {
                let owner = if kind == "fabric" {
                    Owner::Fabric(FabricId(owner))
                } else {
                    Owner::Accessory(AccessoryId(owner))
                };
                (PaletteColorId(color), owner)
            })
            .collect())
    }

    /// Add a color to the palette. Names are unique ignoring capitals.
    pub async fn create_palette_color(
        &self,
        name: &str,
        hex: Option<&str>,
    ) -> Result<PaletteColorId> {
        let id: i64 =
            sqlx::query_scalar("INSERT INTO palette_colors (name, hex) VALUES (?, ?) RETURNING id")
                .bind(name)
                .bind(hex)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| duplicate(e, "palette color", name))?;
        Ok(PaletteColorId(id))
    }

    /// Rename or reshade a palette color: every item using it changes too.
    pub async fn update_palette_color(
        &self,
        id: PaletteColorId,
        name: &str,
        hex: Option<&str>,
    ) -> Result<()> {
        let result = sqlx::query("UPDATE palette_colors SET name = ?, hex = ? WHERE id = ?")
            .bind(name)
            .bind(hex)
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| duplicate(e, "palette color", name))?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "palette color",
                id: id.0,
            });
        }
        Ok(())
    }

    /// Delete a palette color nothing uses. In use: `DbError::InUse` (merge
    /// it into another color instead).
    pub async fn delete_palette_color(&self, id: PaletteColorId) -> Result<()> {
        let result = sqlx::query("DELETE FROM palette_colors WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| in_use(e, "palette color", id.0))?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "palette color",
                id: id.0,
            });
        }
        Ok(())
    }

    /// Pairs of palette colors meant to stay apart, though they look alike.
    pub async fn kept_apart(&self) -> Result<Vec<(PaletteColorId, PaletteColorId)>> {
        let rows: Vec<(i64, i64)> =
            sqlx::query_as("SELECT color_a, color_b FROM palette_kept_apart")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(a, b)| (PaletteColorId(a), PaletteColorId(b)))
            .collect())
    }

    /// Remember that `a` and `b` are meant to differ. Doing it twice is fine.
    pub async fn keep_apart(&self, a: PaletteColorId, b: PaletteColorId) -> Result<()> {
        let (a, b) = if a.0 < b.0 { (a, b) } else { (b, a) };
        if a == b {
            return Ok(());
        }
        sqlx::query("INSERT OR IGNORE INTO palette_kept_apart (color_a, color_b) VALUES (?, ?)")
            .bind(a.0)
            .bind(b.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Merge palette colors into `into`: every item using one of `from` uses
    /// `into` instead (an item that ends up with it twice keeps the first),
    /// and the `from` colors are deleted. All or nothing.
    pub async fn merge_palette_colors(
        &self,
        from: &[PaletteColorId],
        into: PaletteColorId,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM palette_colors WHERE id = ?")
            .bind(into.0)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(DbError::NotFound {
                what: "palette color",
                id: into.0,
            });
        }
        for list in [ColorList::Fabric, ColorList::Accessory] {
            let (table, column) = list.table();
            for color in from.iter().filter(|c| **c != into) {
                sqlx::query(&format!(
                    "UPDATE {table} SET palette_color_id = ? WHERE palette_color_id = ?"
                ))
                .bind(into.0)
                .bind(color.0)
                .execute(&mut *tx)
                .await?;
            }
            // The same color twice in one item: keep the earlier one.
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE rowid IN (
                     SELECT rowid FROM (
                         SELECT rowid, ROW_NUMBER() OVER (
                             PARTITION BY {column}, palette_color_id ORDER BY position
                         ) AS n
                         FROM {table}
                     ) WHERE n > 1
                 )"
            ))
            .execute(&mut *tx)
            .await?;
        }
        for color in from.iter().filter(|c| **c != into) {
            sqlx::query("DELETE FROM palette_colors WHERE id = ?")
                .bind(color.0)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Write a complete copy of the database to `path` (which must not exist
    /// yet). `VACUUM INTO` copies a consistent snapshot even while the app is
    /// running, and the copy is an ordinary SQLite file that can be opened in
    /// place of munchiko.db to go back.
    pub async fn backup_to(&self, path: &std::path::Path) -> Result<()> {
        let path = path
            .to_str()
            .ok_or_else(|| DbError::Corrupt("backup path isn't valid text".into()))?;
        sqlx::query("VACUUM INTO ?")
            .bind(path)
            .execute(&self.pool)
            .await?;
        // An in-memory database (the tests') "succeeds" without writing a
        // file, so never trust the OK alone: a backup that isn't on disk
        // must not let a merge go ahead.
        if !std::path::Path::new(path).exists() {
            return Err(DbError::Corrupt(format!(
                "the backup {path} wasn't written"
            )));
        }
        Ok(())
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
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE fabrics
             SET name = ?, description = ?, keywords = ?, in_stock = ?
             WHERE id = ?",
        )
        .bind(&new.name)
        .bind(&new.description)
        .bind(Json(&new.keywords))
        .bind(new.in_stock)
        .bind(id.0)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "fabric",
                id: id.0,
            });
        }
        set_item_colors(&mut tx, ColorList::Fabric, id.0, &new.colors).await?;
        tx.commit().await?;
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
                 (kind, name, description, colors_from_fabrics, tags, source, brand,
                  status, make_days, quantity)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(new.kind.to_sql())
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.colors_from_fabrics)
        .bind(Json(&new.tags))
        .bind(source)
        .bind(brand)
        .bind(new.status.to_sql())
        .bind(new.make_days)
        .bind(new.quantity)
        .fetch_one(&mut *tx)
        .await?;
        insert_fabric_uses(&mut tx, id, &new.fabrics).await?;
        set_item_colors(&mut tx, ColorList::Accessory, id, &new.colors).await?;

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
            "SELECT id, kind, name, description, colors_from_fabrics, tags, source, brand,
                    status, pair_id, make_days, quantity
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
        let mut colors = load_item_colors(&mut tx, ColorList::Accessory, only).await?;
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
                let colors = colors.remove(&row.id).unwrap_or_default();
                row.into_model(fabrics, colors)
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
             SET kind = ?, name = ?, description = ?, colors_from_fabrics = ?,
                 tags = ?, source = ?, brand = ?,
                 status = ?, make_days = ?, quantity = ?
             WHERE id = ?",
        )
        .bind(new.kind.to_sql())
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.colors_from_fabrics)
        .bind(Json(&new.tags))
        .bind(source)
        .bind(brand)
        .bind(new.status.to_sql())
        .bind(new.make_days)
        .bind(new.quantity)
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
        set_item_colors(&mut tx, ColorList::Accessory, id.0, &new.colors).await?;

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
            "INSERT INTO post_plans (title, event_id, post_date, series_id, backdrop, notes)
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(&new.title)
        .bind(new.event_id.map(|id| id.0))
        .bind(new.post_date)
        .bind(new.series_id.map(|id| id.0))
        .bind(&new.backdrop)
        .bind(&new.notes)
        .fetch_one(&mut *tx)
        .await?;

        insert_looks(&mut tx, id, &new.looks).await?;
        insert_outfit_choices(&mut tx, id, &new.outfits).await?;

        tx.commit().await?;
        Ok(PostPlanId(id))
    }

    pub async fn get_post_plan(&self, id: PostPlanId) -> Result<PostPlan> {
        exactly_one(self.load_post_plans(Some(id)).await?, "post plan", id.0)
    }

    /// All post plans, soonest first; ideas (no date) last.
    pub async fn list_post_plans(&self) -> Result<Vec<PostPlan>> {
        self.load_post_plans(None).await
    }

    async fn load_post_plans(&self, only: Option<PostPlanId>) -> Result<Vec<PostPlan>> {
        let only = only.map(|id| id.0);
        let mut tx = self.pool.begin().await?;

        let rows: Vec<PostPlanRow> = sqlx::query_as(
            "SELECT id, title, event_id, post_date, series_id, backdrop, notes
             FROM post_plans
             WHERE ?1 IS NULL OR id = ?1
             ORDER BY post_date IS NULL, post_date, id",
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
        // Each chosen outfit, with what it knew (NULL if it knew nothing:
        // a LEFT JOIN keeps choices with no rows in post_outfit_known).
        let choice_rows: Vec<(i64, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT c.post_plan_id, c.pet, c.outfit_id, k.accessory_id
             FROM post_outfits c
             LEFT JOIN post_outfit_known k
                 ON k.post_plan_id = c.post_plan_id AND k.pet = c.pet
                AND k.outfit_id = c.outfit_id
             WHERE ?1 IS NULL OR c.post_plan_id = ?1
             ORDER BY c.post_plan_id, c.rowid, k.rowid",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut outfits_by_plan: HashMap<i64, Vec<OutfitChoice>> = HashMap::new();
        for (plan, pet, outfit, piece) in choice_rows {
            let pet = Pet::from_sql(&pet)?;
            let outfit_id = OutfitId(outfit);
            let choices = outfits_by_plan.entry(plan).or_default();
            let index = match choices
                .iter()
                .position(|c| c.pet == pet && c.outfit_id == outfit_id)
            {
                Some(i) => i,
                None => {
                    choices.push(OutfitChoice {
                        pet,
                        outfit_id,
                        known: Vec::new(),
                    });
                    choices.len() - 1
                }
            };
            if let Some(piece) = piece {
                choices[index].known.push(AccessoryId(piece));
            }
        }

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
                let outfits = outfits_by_plan.remove(&row.id).unwrap_or_default();
                row.into_model(looks, outfits)
            })
            .collect())
    }

    /// Save edits to a post plan, replacing its looks.
    pub async fn update_post_plan(&self, id: PostPlanId, new: &NewPostPlan) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE post_plans
             SET title = ?, event_id = ?, post_date = ?, series_id = ?, backdrop = ?, notes = ?
             WHERE id = ?",
        )
        .bind(&new.title)
        .bind(new.event_id.map(|id| id.0))
        .bind(new.post_date)
        .bind(new.series_id.map(|id| id.0))
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
        // Deleting the choices also deletes what they knew (cascade).
        sqlx::query("DELETE FROM post_outfits WHERE post_plan_id = ?")
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        insert_outfit_choices(&mut tx, id.0, &new.outfits).await?;

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

    // -- Series --------------------------------------------------------------

    pub async fn create_series(&self, new: &NewSeries) -> Result<SeriesId> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO series (title, backdrop, notes) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(&new.title)
        .bind(&new.backdrop)
        .bind(&new.notes)
        .fetch_one(&self.pool)
        .await?;
        Ok(SeriesId(id))
    }

    /// Turn a post into the first post of a new series: the series is named
    /// `title` and takes over the post's backdrop (the shared setup).
    pub async fn series_from_post(&self, post: PostPlanId, title: &str) -> Result<SeriesId> {
        let mut tx = self.pool.begin().await?;
        let backdrop: Option<String> =
            sqlx::query_scalar("SELECT backdrop FROM post_plans WHERE id = ?")
                .bind(post.0)
                .fetch_optional(&mut *tx)
                .await?;
        let backdrop = backdrop.ok_or(DbError::NotFound {
            what: "post plan",
            id: post.0,
        })?;
        let id: i64 =
            sqlx::query_scalar("INSERT INTO series (title, backdrop) VALUES (?, ?) RETURNING id")
                .bind(title)
                .bind(&backdrop)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("UPDATE post_plans SET series_id = ?, backdrop = '' WHERE id = ?")
            .bind(id)
            .bind(post.0)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(SeriesId(id))
    }

    pub async fn get_series(&self, id: SeriesId) -> Result<Series> {
        let row: Option<SeriesRow> =
            sqlx::query_as("SELECT id, title, backdrop, notes FROM series WHERE id = ?")
                .bind(id.0)
                .fetch_optional(&self.pool)
                .await?;
        row.map(SeriesRow::into_model).ok_or(DbError::NotFound {
            what: "series",
            id: id.0,
        })
    }

    /// All series, by title.
    pub async fn list_series(&self) -> Result<Vec<Series>> {
        let rows: Vec<SeriesRow> =
            sqlx::query_as("SELECT id, title, backdrop, notes FROM series ORDER BY title, id")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(SeriesRow::into_model).collect())
    }

    pub async fn update_series(&self, id: SeriesId, new: &NewSeries) -> Result<()> {
        let result =
            sqlx::query("UPDATE series SET title = ?, backdrop = ?, notes = ? WHERE id = ?")
                .bind(&new.title)
                .bind(&new.backdrop)
                .bind(&new.notes)
                .bind(id.0)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "series",
                id: id.0,
            });
        }
        Ok(())
    }

    /// Delete a series but keep its posts: they become standalone posts, and
    /// any without a backdrop of their own get the series' backdrop.
    pub async fn delete_series(&self, id: SeriesId) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE post_plans
             SET backdrop = (SELECT backdrop FROM series WHERE id = ?1)
             WHERE series_id = ?1 AND backdrop = ''",
        )
        .bind(id.0)
        .execute(&mut *tx)
        .await?;
        // ON DELETE SET NULL takes the posts out of the series.
        let result = sqlx::query("DELETE FROM series WHERE id = ?")
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "series",
                id: id.0,
            });
        }
        tx.commit().await?;
        Ok(())
    }

    // -- Outfits -------------------------------------------------------------

    /// Saves the outfit and its pieces together. If any piece doesn't exist,
    /// nothing is saved (the foreign key refuses it).
    pub async fn create_outfit(&self, new: &NewOutfit) -> Result<OutfitId> {
        let mut tx = self.pool.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO outfits (name, description) VALUES (?, ?) RETURNING id",
        )
        .bind(&new.name)
        .bind(&new.description)
        .fetch_one(&mut *tx)
        .await?;
        insert_pieces(&mut tx, id, &new.pieces).await?;
        tx.commit().await?;
        Ok(OutfitId(id))
    }

    pub async fn get_outfit(&self, id: OutfitId) -> Result<Outfit> {
        exactly_one(self.load_outfits(Some(id)).await?, "outfit", id.0)
    }

    /// All outfits, sorted by name.
    pub async fn list_outfits(&self) -> Result<Vec<Outfit>> {
        self.load_outfits(None).await
    }

    async fn load_outfits(&self, only: Option<OutfitId>) -> Result<Vec<Outfit>> {
        let only = only.map(|id| id.0);
        let mut tx = self.pool.begin().await?;
        let rows: Vec<OutfitRow> = sqlx::query_as(
            "SELECT id, name, description FROM outfits
             WHERE ?1 IS NULL OR id = ?1
             ORDER BY name, id",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;
        // (outfit, piece) pairs in the order the pieces were added.
        let pieces: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT outfit_id, accessory_id FROM outfit_pieces
             WHERE ?1 IS NULL OR outfit_id = ?1
             ORDER BY outfit_id, rowid",
        )
        .bind(only)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut by_outfit: HashMap<i64, Vec<AccessoryId>> = HashMap::new();
        for (outfit, piece) in pieces {
            by_outfit
                .entry(outfit)
                .or_default()
                .push(AccessoryId(piece));
        }
        Ok(rows
            .into_iter()
            .map(|row| Outfit {
                id: OutfitId(row.id),
                pieces: by_outfit.remove(&row.id).unwrap_or_default(),
                name: row.name,
                description: row.description,
            })
            .collect())
    }

    /// Save edits to an outfit, replacing its list of pieces.
    pub async fn update_outfit(&self, id: OutfitId, new: &NewOutfit) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("UPDATE outfits SET name = ?, description = ? WHERE id = ?")
            .bind(&new.name)
            .bind(&new.description)
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "outfit",
                id: id.0,
            });
        }
        sqlx::query("DELETE FROM outfit_pieces WHERE outfit_id = ?")
            .bind(id.0)
            .execute(&mut *tx)
            .await?;
        insert_pieces(&mut tx, id.0, &new.pieces).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Delete an outfit, its piece list and its photo records. The pieces
    /// themselves, and posts that wore them, are untouched.
    pub async fn delete_outfit(&self, id: OutfitId) -> Result<()> {
        let result = sqlx::query("DELETE FROM outfits WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "outfit",
                id: id.0,
            });
        }
        Ok(())
    }

    // -- Photos --------------------------------------------------------------
    // Only the records; the files are handled by `crate::photos::PhotoStore`.

    /// Record a new photo for `owner` (which must exist) and return it, so
    /// the caller can save the file under the new id.
    pub async fn add_photo(&self, owner: PhotoOwner, image_type: ImageType) -> Result<Photo> {
        let (fabric, accessory, post_plan, outfit) = owner_columns(owner);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO photos (fabric_id, accessory_id, post_plan_id, outfit_id, content_type)
             VALUES (?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(fabric)
        .bind(accessory)
        .bind(post_plan)
        .bind(outfit)
        .bind(image_type.mime())
        .fetch_one(&self.pool)
        .await?;
        Ok(Photo {
            id: PhotoId(id),
            owner,
            image_type,
        })
    }

    pub async fn get_photo(&self, id: PhotoId) -> Result<Photo> {
        let row: Option<PhotoRow> = sqlx::query_as(
            "SELECT id, fabric_id, accessory_id, post_plan_id, outfit_id, content_type
             FROM photos WHERE id = ?",
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await?;
        row.ok_or(DbError::NotFound {
            what: "photo",
            id: id.0,
        })?
        .into_model()
    }

    /// One owner's photos, oldest first (the first one is its thumbnail).
    pub async fn list_photos(&self, owner: PhotoOwner) -> Result<Vec<Photo>> {
        let (fabric, accessory, post_plan, outfit) = owner_columns(owner);
        // `IS` is `=` that also treats NULL = NULL as true, so the
        // columns that aren't this owner's kind match their NULLs.
        let rows: Vec<PhotoRow> = sqlx::query_as(
            "SELECT id, fabric_id, accessory_id, post_plan_id, outfit_id, content_type
             FROM photos
             WHERE fabric_id IS ? AND accessory_id IS ? AND post_plan_id IS ?
                   AND outfit_id IS ?
             ORDER BY id",
        )
        .bind(fabric)
        .bind(accessory)
        .bind(post_plan)
        .bind(outfit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(PhotoRow::into_model).collect()
    }

    /// Every photo, oldest first, for list pages that show thumbnails.
    pub async fn list_all_photos(&self) -> Result<Vec<Photo>> {
        let rows: Vec<PhotoRow> = sqlx::query_as(
            "SELECT id, fabric_id, accessory_id, post_plan_id, outfit_id, content_type
             FROM photos ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(PhotoRow::into_model).collect()
    }

    pub async fn delete_photo(&self, id: PhotoId) -> Result<()> {
        let result = sqlx::query("DELETE FROM photos WHERE id = ?")
            .bind(id.0)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound {
                what: "photo",
                id: id.0,
            });
        }
        Ok(())
    }
}

/// The owner as the four owner columns, exactly one of them filled in.
type OwnerColumns = (Option<i64>, Option<i64>, Option<i64>, Option<i64>);

fn owner_columns(owner: PhotoOwner) -> OwnerColumns {
    match owner {
        PhotoOwner::Fabric(id) => (Some(id.0), None, None, None),
        PhotoOwner::Accessory(id) => (None, Some(id.0), None, None),
        PhotoOwner::PostPlan(id) => (None, None, Some(id.0), None),
        PhotoOwner::Outfit(id) => (None, None, None, Some(id.0)),
    }
}

/// Insert an outfit's pieces in order, inside the caller's transaction.
async fn insert_pieces(
    conn: &mut SqliteConnection,
    outfit_id: i64,
    pieces: &[AccessoryId],
) -> Result<()> {
    for piece in pieces {
        // OR IGNORE: the same piece listed twice is saved once.
        sqlx::query("INSERT OR IGNORE INTO outfit_pieces (outfit_id, accessory_id) VALUES (?, ?)")
            .bind(outfit_id)
            .bind(piece.0)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
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
            tags: vec![],
            fabrics: fabrics
                .iter()
                .map(|&(fabric_id, role)| FabricUse { fabric_id, role })
                .collect(),
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            make_days: None,
            quantity: 1,
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
    async fn an_idea_needs_no_date_or_event_but_must_be_called_something() {
        let db = Repository::in_memory().await.unwrap();
        let fossil = db.create_accessory(&bow("Fossil bow", &[])).await.unwrap();
        let idea = NewPostPlan {
            title: "Pokémon fossil exhibit".into(),
            event_id: None,
            post_date: None,
            series_id: None,
            looks: vec![Look {
                pet: Pet::Momo,
                accessory_id: fossil,
            }],
            outfits: vec![],
            backdrop: "museum props".into(),
            notes: String::new(),
        };
        let id = db.create_post_plan(&idea).await.unwrap();
        let loaded = db.get_post_plan(id).await.unwrap();
        assert!(loaded.is_idea());
        assert_eq!(loaded.title, "Pokémon fossil exhibit");
        assert_eq!(loaded.event_id, None);

        // No title and no event: the database itself refuses it.
        let nameless = NewPostPlan {
            title: String::new(),
            ..idea.clone()
        };
        assert!(db.create_post_plan(&nameless).await.is_err());

        // Ideas sort after dated posts.
        let dated = db
            .create_post_plan(&NewPostPlan {
                title: "Rock day".into(),
                post_date: Some(d(2026, 11, 2)),
                ..idea
            })
            .await
            .unwrap();
        let order: Vec<PostPlanId> = db
            .list_post_plans()
            .await
            .unwrap()
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(order, vec![dated, id]);
    }

    #[tokio::test]
    async fn a_series_takes_over_a_posts_backdrop_and_gives_it_back() {
        let db = Repository::in_memory().await.unwrap();
        let earth = db
            .create_event(&event("Earth Day", d(2027, 4, 22), None))
            .await
            .unwrap();
        let first = db
            .create_post_plan(&NewPostPlan {
                title: "Momo's Earth Day".into(),
                event_id: Some(earth),
                post_date: Some(d(2027, 4, 20)),
                series_id: None,
                looks: vec![],
                outfits: vec![],
                backdrop: "moss and ferns".into(),
                notes: String::new(),
            })
            .await
            .unwrap();

        // "+ Another post from this shoot": the backdrop moves to the series.
        let series = db.series_from_post(first, "Earth Day").await.unwrap();
        let s = db.get_series(series).await.unwrap();
        assert_eq!(s.backdrop, "moss and ferns");
        let post = db.get_post_plan(first).await.unwrap();
        assert_eq!(post.series_id, Some(series));
        assert_eq!(post.backdrop, "");
        assert_eq!(db.list_series().await.unwrap().len(), 1);

        // Deleting the series keeps the post, with the backdrop back.
        db.delete_series(series).await.unwrap();
        let post = db.get_post_plan(first).await.unwrap();
        assert_eq!(post.series_id, None);
        assert_eq!(post.backdrop, "moss and ferns");
        assert!(matches!(
            db.get_series(series).await,
            Err(DbError::NotFound { what: "series", .. })
        ));
    }

    /// An in-memory database as it was at schema `version`: only the
    /// migrations up to that one have run. For testing later migrations.
    async fn database_at_version(version: i64) -> SqlitePool {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .unwrap();
        let up_to = Migrator {
            migrations: MIGRATOR
                .migrations
                .iter()
                .filter(|m| m.version <= version)
                .cloned()
                .collect(),
            ..Migrator::DEFAULT
        };
        up_to.run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn migration_0009_keeps_sets_fabrics_looks_and_photos() {
        let pool = database_at_version(8).await;
        // A matching set (pointing at each other), one bow with two fabrics in
        // a set order, a post wearing both, and a photo of each bow.
        sqlx::raw_sql(
            "INSERT INTO fabrics (id, name) VALUES (1, 'Leaf print'), (2, 'Moss velvet');
             INSERT INTO accessories (id, kind, name, source, status, tags) VALUES
                 (1, 'bow', 'Leaf bow', 'handmade', 'ready', '[\"batik\"]'),
                 (2, 'scarf', 'Moss scarf', 'bought', 'ready', '[]');
             UPDATE accessories SET pair_id = 2 WHERE id = 1;
             UPDATE accessories SET pair_id = 1 WHERE id = 2;
             INSERT INTO accessory_fabrics (accessory_id, fabric_id, role)
                 VALUES (1, 2, 'middle'), (1, 1, 'main');
             INSERT INTO events (id, name, kind, start_date) VALUES (1, 'Earth Day', 'notable_day', '2027-04-22');
             INSERT INTO post_plans (id, event_id, post_date) VALUES (7, 1, '2027-04-22');
             INSERT INTO looks (post_plan_id, pet, accessory_id) VALUES (7, 'momo', 2), (7, 'mochi', 1);
             INSERT INTO photos (id, accessory_id, content_type) VALUES (4, 1, 'image/jpeg'), (9, 2, 'image/png');",
        )
        .execute(&pool)
        .await
        .unwrap();

        let db = Repository::migrate(pool).await.unwrap();

        let leaf = db.get_accessory(AccessoryId(1)).await.unwrap();
        let moss = db.get_accessory(AccessoryId(2)).await.unwrap();
        assert_eq!(leaf.pair_id, Some(AccessoryId(2)), "the set survives");
        assert_eq!(moss.pair_id, Some(AccessoryId(1)));
        assert_eq!(leaf.tags, ["batik"]);
        let uses: Vec<(i64, FabricRole)> = leaf
            .fabrics
            .iter()
            .map(|u| (u.fabric_id.0, u.role))
            .collect();
        assert_eq!(
            uses,
            [(2, FabricRole::Middle), (1, FabricRole::Main)],
            "same order"
        );
        let looks: Vec<(Pet, i64)> = db
            .get_post_plan(PostPlanId(7))
            .await
            .unwrap()
            .looks
            .iter()
            .map(|l| (l.pet, l.accessory_id.0))
            .collect();
        assert_eq!(looks, [(Pet::Momo, 2), (Pet::Mochi, 1)]);
        assert_eq!(
            db.get_photo(PhotoId(9)).await.unwrap().owner,
            PhotoOwner::Accessory(AccessoryId(2))
        );

        // The new kinds are allowed, and the foreign keys still protect things.
        let mut crown = bow("Glinda crown", &[]);
        crown.kind = AccessoryKind::Hat;
        let crown = db.create_accessory(&crown).await.unwrap();
        assert_eq!(
            db.get_accessory(crown).await.unwrap().kind,
            AccessoryKind::Hat
        );
        assert!(matches!(
            db.delete_accessory(AccessoryId(1)).await,
            Err(DbError::InUse { .. })
        ));
        let problems: Vec<(String,)> =
            sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[tokio::test]
    async fn outfits_keep_their_pieces_in_order() {
        let db = Repository::in_memory().await.unwrap();
        let crown = db
            .create_accessory(&bow("Glinda crown", &[]))
            .await
            .unwrap();
        let dress = db
            .create_accessory(&bow("Glinda dress", &[]))
            .await
            .unwrap();
        let wand = db.create_accessory(&bow("Star wand", &[])).await.unwrap();

        let glinda = NewOutfit {
            name: "Glinda".into(),
            description: "Wicked night".into(),
            pieces: vec![dress, crown, wand, crown],
        };
        let id = db.create_outfit(&glinda).await.unwrap();
        let saved = db.get_outfit(id).await.unwrap();
        assert_eq!(
            saved.pieces,
            [dress, crown, wand],
            "in order, repeat dropped"
        );

        // The same wand in a second outfit.
        let fairy = db
            .create_outfit(&NewOutfit {
                name: "Fairy".into(),
                description: String::new(),
                pieces: vec![wand],
            })
            .await
            .unwrap();
        let names: Vec<String> = db
            .list_outfits()
            .await
            .unwrap()
            .into_iter()
            .map(|o| o.name)
            .collect();
        assert_eq!(names, ["Fairy", "Glinda"]);

        // Editing replaces the pieces; an unknown piece saves nothing.
        let edited = NewOutfit {
            pieces: vec![crown, dress],
            ..glinda.clone()
        };
        db.update_outfit(id, &edited).await.unwrap();
        assert_eq!(db.get_outfit(id).await.unwrap().pieces, [crown, dress]);
        let bad = NewOutfit {
            pieces: vec![crown, AccessoryId(99)],
            ..glinda
        };
        assert!(db.update_outfit(id, &bad).await.is_err());
        assert_eq!(db.get_outfit(id).await.unwrap().pieces, [crown, dress]);

        // Deleting a piece takes it out of its outfits; deleting an outfit
        // leaves its pieces and takes its photos.
        db.delete_accessory(wand).await.unwrap();
        assert!(db.get_outfit(fairy).await.unwrap().pieces.is_empty());
        let photo = db
            .add_photo(PhotoOwner::Outfit(id), ImageType::Jpeg)
            .await
            .unwrap();
        assert_eq!(
            db.list_photos(PhotoOwner::Outfit(id)).await.unwrap(),
            std::slice::from_ref(&photo)
        );
        assert_eq!(
            db.get_photo(photo.id).await.unwrap().owner,
            PhotoOwner::Outfit(id)
        );
        db.delete_outfit(id).await.unwrap();
        assert!(db.get_photo(photo.id).await.is_err());
        assert!(db.get_accessory(crown).await.is_ok());
        assert!(matches!(
            db.delete_outfit(id).await,
            Err(DbError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn outfit_choices_are_saved_with_the_post() {
        let db = Repository::in_memory().await.unwrap();
        let crown = db.create_accessory(&bow("Crown", &[])).await.unwrap();
        let glinda = db
            .create_outfit(&NewOutfit {
                name: "Glinda".into(),
                description: String::new(),
                pieces: vec![crown],
            })
            .await
            .unwrap();
        let empty = db
            .create_outfit(&NewOutfit {
                name: "Not started".into(),
                description: String::new(),
                pieces: vec![],
            })
            .await
            .unwrap();
        let choices = vec![
            OutfitChoice {
                pet: Pet::Momo,
                outfit_id: glinda,
                known: vec![crown],
            },
            // An outfit with no pieces yet is still remembered.
            OutfitChoice {
                pet: Pet::Mochi,
                outfit_id: empty,
                known: vec![],
            },
        ];
        let id = db
            .create_post_plan(&NewPostPlan {
                title: "Wicked".into(),
                event_id: None,
                post_date: None,
                series_id: None,
                looks: vec![Look {
                    pet: Pet::Momo,
                    accessory_id: crown,
                }],
                outfits: choices.clone(),
                backdrop: String::new(),
                notes: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(db.get_post_plan(id).await.unwrap().outfits, choices);

        // Deleting a piece removes it from what a choice knew.
        let plan = db.get_post_plan(id).await.unwrap();
        db.update_post_plan(
            id,
            &NewPostPlan {
                looks: vec![],
                ..NewPostPlan::from(plan)
            },
        )
        .await
        .unwrap();
        db.delete_accessory(crown).await.unwrap();
        assert!(
            db.get_post_plan(id).await.unwrap().outfits[0]
                .known
                .is_empty()
        );
    }

    #[tokio::test]
    async fn migration_0011_keeps_photo_ids_and_never_reuses_them() {
        let pool = database_at_version(10).await;
        // Photos 1-3, then 3 deleted: the next new photo must still be 4,
        // or a browser that cached /photos/3 would show the old picture.
        sqlx::raw_sql(
            "INSERT INTO fabrics (id, name) VALUES (1, 'Leaf print');
             INSERT INTO accessories (id, kind, name, source, status)
                 VALUES (1, 'bow', 'Leaf bow', 'handmade', 'ready');
             INSERT INTO photos (fabric_id, accessory_id, content_type) VALUES
                 (1, NULL, 'image/jpeg'), (NULL, 1, 'image/png'), (1, NULL, 'image/gif');
             DELETE FROM photos WHERE id = 3;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let db = Repository::migrate(pool).await.unwrap();

        let owners: Vec<(i64, PhotoOwner)> = db
            .list_all_photos()
            .await
            .unwrap()
            .into_iter()
            .map(|p| (p.id.0, p.owner))
            .collect();
        assert_eq!(
            owners,
            [
                (1, PhotoOwner::Fabric(FabricId(1))),
                (2, PhotoOwner::Accessory(AccessoryId(1))),
            ]
        );
        let next = db
            .add_photo(PhotoOwner::Fabric(FabricId(1)), ImageType::Jpeg)
            .await
            .unwrap();
        assert_eq!(next.id, PhotoId(4));
        // Still exactly one owner per photo.
        assert!(
            sqlx::query("INSERT INTO photos (fabric_id, accessory_id, content_type) VALUES (1, 1, 'image/jpeg')")
                .execute(&db.pool)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn palette_colors_are_shared_and_merged_with_a_backup_first() {
        // A real file: an in-memory database can't be copied to disk (and
        // `backup_to` says so rather than pretending).
        let dir = tempfile::tempdir().unwrap();
        let db = Repository::connect(&format!(
            "sqlite://{}",
            dir.path().join("live.db").display()
        ))
        .await
        .unwrap();
        assert!(
            Repository::in_memory()
                .await
                .unwrap()
                .backup_to(&dir.path().join("nothing.db"))
                .await
                .is_err()
        );
        let fabric = |name: &str, colors: Vec<Color>| NewFabric {
            name: name.into(),
            description: String::new(),
            colors,
            keywords: vec![],
            in_stock: true,
        };
        // "Black" and "black" are one palette color; the palette's shade wins.
        let velvet = db
            .create_fabric(&fabric("Velvet", vec![Color::with_hex("black", "#101010")]))
            .await
            .unwrap();
        let satin = db
            .create_fabric(&fabric(
                "Satin",
                vec![
                    Color::with_hex("Black", "#141414"),
                    Color::with_hex("jet", "#0a0a0a"),
                ],
            ))
            .await
            .unwrap();
        let palette = db.list_palette().await.unwrap();
        assert_eq!(palette.len(), 2);
        assert_eq!(
            db.get_fabric(satin).await.unwrap().colors[0],
            Color::with_hex("black", "#101010")
        );

        // Reshading a palette color changes every item using it.
        let black = palette.iter().find(|c| c.name == "black").unwrap().id;
        db.update_palette_color(black, "black", Some("#111111"))
            .await
            .unwrap();
        for id in [velvet, satin] {
            assert_eq!(
                db.get_fabric(id).await.unwrap().colors[0].hex.as_deref(),
                Some("#111111")
            );
        }
        // Names stay unique, ignoring capitals.
        assert!(matches!(
            db.create_palette_color("JET", None).await,
            Err(DbError::Duplicate { .. })
        ));
        // In use: can't be deleted.
        assert!(matches!(
            db.delete_palette_color(black).await,
            Err(DbError::InUse { .. })
        ));

        // Merge jet into black, after a backup.
        let jet = db
            .list_palette()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.name == "jet")
            .unwrap()
            .id;
        let copy = dir.path().join("before.db");
        db.backup_to(&copy).await.unwrap();
        db.merge_palette_colors(&[jet], black).await.unwrap();
        assert_eq!(
            db.get_fabric(satin).await.unwrap().colors,
            [Color::with_hex("black", "#111111")],
            "black twice is kept once"
        );
        assert_eq!(db.list_palette().await.unwrap().len(), 1);
        let uses = db.palette_uses().await.unwrap();
        assert_eq!(uses.len(), 2);
        // The backup still has jet.
        let old = Repository::connect(&format!("sqlite://{}", copy.display()))
            .await
            .unwrap();
        assert_eq!(old.get_fabric(satin).await.unwrap().colors.len(), 2);
        assert!(db.backup_to(&copy).await.is_err(), "won't overwrite");

        // An unused color can go.
        let spare = db.create_palette_color("spare", None).await.unwrap();
        db.delete_palette_color(spare).await.unwrap();
    }

    #[tokio::test]
    async fn migration_0013_moves_colors_into_the_palette_losing_nothing() {
        let pool = database_at_version(12).await;
        sqlx::raw_sql(
            r##"INSERT INTO fabrics (id, name, colors) VALUES
                 (1, 'Velvet', '[{"name":"pink","hex":"#d37cab"},{"name":"black","hex":"#101010"}]'),
                 (2, 'Satin',  '[{"name":"pink","hex":"#d37cab"},{"name":"black"}]'),
                 (3, 'Tulle',  '[{"name":"pink","hex":"#ffcef8"},{"name":"sparkly"}]'),
                 (4, 'Plain',  '[]');
               INSERT INTO accessories (id, kind, name, source, status, colors) VALUES
                 (1, 'bow', 'Pink bow', 'handmade', 'ready', '[{"name":"Pink","hex":"#d37cab"}]');"##,
        )
        .execute(&pool)
        .await
        .unwrap();
        let db = Repository::migrate(pool).await.unwrap();

        let names: Vec<(String, Option<String>)> = db
            .list_palette()
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.name, c.hex))
            .collect();
        assert_eq!(
            names,
            [
                ("black".to_string(), Some("#101010".to_string())),
                ("pink".to_string(), Some("#d37cab".to_string())),
                ("pink (2)".to_string(), Some("#ffcef8".to_string())),
                ("sparkly".to_string(), None),
            ],
            "most used pink first; the other gets (2)"
        );
        assert_eq!(
            db.get_fabric(FabricId(2)).await.unwrap().colors,
            [
                Color::with_hex("pink", "#d37cab"),
                Color::with_hex("black", "#101010")
            ],
            "a shadeless black takes black's shade; order kept"
        );
        assert_eq!(
            db.get_fabric(FabricId(3)).await.unwrap().colors,
            [
                Color::with_hex("pink (2)", "#ffcef8"),
                Color::named("sparkly")
            ]
        );
        assert!(db.get_fabric(FabricId(4)).await.unwrap().colors.is_empty());
        assert_eq!(
            db.get_accessory(AccessoryId(1)).await.unwrap().colors,
            [Color::with_hex("pink", "#d37cab")],
            "\"Pink\" is pink"
        );
    }

    #[tokio::test]
    async fn quantity_starts_at_one_and_is_saved() {
        // An accessory from before migration 0010 counts as one.
        let pool = database_at_version(9).await;
        sqlx::raw_sql(
            "INSERT INTO accessories (id, kind, name, source, status)
                 VALUES (1, 'bow', 'Leaf bow', 'handmade', 'ready');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let db = Repository::migrate(pool).await.unwrap();
        assert_eq!(db.get_accessory(AccessoryId(1)).await.unwrap().quantity, 1);

        let mut twins = bow("Twin bows", &[]);
        twins.quantity = 2;
        let id = db.create_accessory(&twins).await.unwrap();
        assert_eq!(db.get_accessory(id).await.unwrap().quantity, 2);
        twins.quantity = 3;
        db.update_accessory(id, &twins).await.unwrap();
        assert_eq!(db.get_accessory(id).await.unwrap().quantity, 3);

        // The table refuses zero, even if a caller forgets to check.
        twins.quantity = 0;
        assert!(db.create_accessory(&twins).await.is_err());
    }

    #[tokio::test]
    async fn migration_0006_keeps_posts_looks_and_photos() {
        // A database as it was before 0006.
        let pool = database_at_version(5).await;

        // A post with two looks (order matters) and a photo, written the old way.
        sqlx::raw_sql(
            "INSERT INTO events (id, name, kind, start_date) VALUES (1, 'Earth Day', 'notable_day', '2027-04-22');
             INSERT INTO accessories (id, kind, name, source, status) VALUES
                 (1, 'bow', 'Leaf bow', 'handmade', 'ready'),
                 (2, 'bow', 'Globe bow', 'handmade', 'ready');
             INSERT INTO post_plans (id, event_id, post_date, backdrop, notes)
                 VALUES (7, 1, '2027-04-22', 'moss and ferns', 'combined post');
             INSERT INTO looks (post_plan_id, pet, accessory_id) VALUES (7, 'momo', 2), (7, 'mochi', 1);
             INSERT INTO photos (id, post_plan_id, content_type) VALUES (3, 7, 'image/jpeg');",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Now run 0006 (and anything after it).
        let db = Repository::migrate(pool).await.unwrap();

        let plan = db.get_post_plan(PostPlanId(7)).await.unwrap();
        assert_eq!(plan.event_id, Some(EventId(1)));
        assert_eq!(plan.post_date, Some(d(2027, 4, 22)));
        assert_eq!(plan.title, "");
        assert_eq!(plan.backdrop, "moss and ferns");
        let looks: Vec<(Pet, i64)> = plan
            .looks
            .iter()
            .map(|l| (l.pet, l.accessory_id.0))
            .collect();
        assert_eq!(
            looks,
            vec![(Pet::Momo, 2), (Pet::Mochi, 1)],
            "same looks, same order"
        );
        let photo = db.get_photo(PhotoId(3)).await.unwrap();
        assert_eq!(photo.owner, PhotoOwner::PostPlan(PostPlanId(7)));

        // And the foreign keys still work: the accessory is still protected.
        assert!(matches!(
            db.delete_accessory(AccessoryId(1)).await,
            Err(DbError::InUse { .. })
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
            title: String::new(),
            event_id: Some(halloween),
            post_date: Some(d(2026, 10, 31)),
            series_id: None,
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
            outfits: vec![],
            backdrop: "hay bales".into(),
            notes: String::new(),
        };
        let id = db.create_post_plan(&new).await.unwrap();

        let loaded = db.get_post_plan(id).await.unwrap();
        assert_eq!(loaded.looks, new.looks);
        assert_eq!(loaded.post_date, Some(d(2026, 10, 31)));

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
                title: String::new(),
                event_id: Some(halloween),
                post_date: Some(d(2026, 10, 31)),
                series_id: None,
                looks: vec![Look {
                    pet: Pet::Mochi,
                    accessory_id: pumpkin,
                }],
                outfits: vec![],
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
            title: String::new(),
            event_id: Some(thanksgiving_2026.id),
            post_date: Some(d(2026, 11, 26)),
            series_id: None,
            looks: vec![],
            outfits: vec![],
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
    async fn photos_belong_to_one_owner_and_go_when_it_goes() {
        let db = Repository::in_memory().await.unwrap();
        let velvet = db.create_fabric(&fabric("Velvet", true)).await.unwrap();
        let satin = db.create_fabric(&fabric("Satin", true)).await.unwrap();
        let bow = db.create_accessory(&bow("Bow", &[])).await.unwrap();

        let first = db
            .add_photo(PhotoOwner::Fabric(velvet), ImageType::Jpeg)
            .await
            .unwrap();
        let second = db
            .add_photo(PhotoOwner::Fabric(velvet), ImageType::Png)
            .await
            .unwrap();
        db.add_photo(PhotoOwner::Fabric(satin), ImageType::Jpeg)
            .await
            .unwrap();
        let bow_photo = db
            .add_photo(PhotoOwner::Accessory(bow), ImageType::Webp)
            .await
            .unwrap();

        // Oldest first, and only this owner's.
        assert_eq!(
            db.list_photos(PhotoOwner::Fabric(velvet)).await.unwrap(),
            vec![first.clone(), second]
        );
        assert_eq!(db.get_photo(bow_photo.id).await.unwrap(), bow_photo);
        assert_eq!(db.list_all_photos().await.unwrap().len(), 4);

        // Deleting the fabric takes its photo records with it (CASCADE).
        db.delete_fabric(velvet).await.unwrap();
        assert!(matches!(
            db.get_photo(first.id).await,
            Err(DbError::NotFound { what: "photo", .. })
        ));
        assert_eq!(db.list_all_photos().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn photo_ids_are_never_reused() {
        let db = Repository::in_memory().await.unwrap();
        let velvet = db.create_fabric(&fabric("Velvet", true)).await.unwrap();
        let owner = PhotoOwner::Fabric(velvet);
        let old = db.add_photo(owner, ImageType::Jpeg).await.unwrap();
        db.delete_photo(old.id).await.unwrap();
        // A browser may still have /photos/<old id> cached.
        let new = db.add_photo(owner, ImageType::Jpeg).await.unwrap();
        assert!(new.id > old.id);
    }

    #[tokio::test]
    async fn a_photo_needs_an_owner_that_exists() {
        let db = Repository::in_memory().await.unwrap();
        let result = db
            .add_photo(PhotoOwner::Fabric(FabricId(99)), ImageType::Jpeg)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn migration_0004_turns_color_words_into_objects() {
        // A database from before 0004 (colors were plain words), migrated all
        // the way: 0004 makes them objects, 0013 moves them into the palette.
        let pool = database_at_version(3).await;
        sqlx::query(
            "INSERT INTO fabrics (name, description, colors, keywords, in_stock)
             VALUES ('Old', '', '[\"baby pink\",\"white\"]', '[]', 1),
                    ('Plain', '', '[]', '[]', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let db = Repository::migrate(pool).await.unwrap();

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
                title: String::new(),
                event_id: Some(halloween),
                post_date: Some(d(2026, 10, 31)),
                series_id: None,
                looks: vec![Look {
                    pet: Pet::Mochi,
                    accessory_id: pumpkin,
                }],
                outfits: vec![],
                backdrop: String::new(),
                notes: String::new(),
            })
            .await
            .unwrap();

        let edited = NewPostPlan {
            title: String::new(),
            event_id: Some(other),
            post_date: Some(d(2026, 10, 27)),
            series_id: None,
            looks: vec![Look {
                pet: Pet::Momo,
                accessory_id: bat,
            }],
            outfits: vec![],
            backdrop: "black velvet".into(),
            notes: "Momo solo".into(),
        };
        db.update_post_plan(plan, &edited).await.unwrap();
        let loaded = db.get_post_plan(plan).await.unwrap();
        assert_eq!(loaded.event_id, Some(other));
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
