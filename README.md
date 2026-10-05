# Munchiko Calendar 🐶🐱🎀

A content planner for **Mochi & Momo's** social media account, written in Rust.

The account posts photos of a dog and a cat in (mostly handmade) bows and
costumes, set against a backdrop for a holiday or notable day — Christmas,
Earth Day, the Ides of March, Shark Week. This tool doesn't make content; it
makes sure the content is *ready on time*. Given a target post date, it works
backward to tell you when to buy fabric, make each bow, build the backdrop and
take the photos.

## How the planning works

```
 buy fabric ──► make bow ──┐
                           ├──► ready day ──► photo shoot ──(editing buffer)──► publish
 order bought item ────────┤
 build backdrop ───────────┘
```

- Everything must be finished the day **before** the shoot.
- Bows, bought items and the backdrop happen **in parallel**; whichever must
  start earliest is the **critical path**.
- **Slack** = days between today and that earliest start. Negative slack means
  the post is already behind schedule.
- Lead times (editing buffer, backdrop build, shipping, fabric shopping) are
  configurable, and each bow can override how long it takes to make.

Example — Halloween (post Oct 31, default lead times):

| When | Task |
|---|---|
| Oct 20 | Order Ghost scarf (bought) |
| Oct 21 – 23 | Buy fabric: Orange velvet (out of stock) |
| Oct 24 – 27 | Make Pumpkin bow for Mochi |
| Oct 26 – 27 | Make Bat bow for Momo · Build the backdrop |
| Oct 28 | Photo shoot |
| Oct 31 | Publish |

## Data model

| Model | Key fields |
|---|---|
| **Fabric** | name, description, colors, keywords, pairings, in stock |
| **Accessory** (bow / scarf / costume) | name, description, colors, fabrics (main/middle/accent), handmade or bought (brand), status (idea → in progress → ready), matching pair |
| **Event** | name, description, type, start/end date, keywords |
| **Post plan** | event, post date, who wears what, backdrop idea |

Bows are separate items that can be linked as a matching Mochi & Momo set.

## Holidays and notable days

`data/holidays.toml` lists about 45 holidays, fun days, seasons and Mochi &
Momo's birthdays and adoption days. Each entry has one date rule:

| Rule | Example |
|---|---|
| `date = "10-31"` | Halloween |
| `nth_weekday = { n = 4, weekday = "thu", month = 11 }` | Thanksgiving (`n = -1` = last) |
| `easter_offset = -47` | Mardi Gras (Easter is calculated) |
| `by_year = { 2027 = "02-06" }` | Lunar New Year, Shark Week: looked up each year |

Optional `days = 7` makes it multi-day, and `offset_days = 1` shifts it
(Black Friday = Thanksgiving + 1). The app re-reads the file on every start
and syncs this year's and next year's dates into the calendar: new holidays
are added, changed ones updated in place, removed ones deleted (unless a post
is already planned for them). Outings you enter yourself are never touched.

## Storage

Data lives in SQLite (`munchiko.db` by default; set `DATABASE_URL` to change
it). The schema is created and upgraded by numbered migrations in
`migrations/`, which run automatically when the app starts.

| Table | Holds |
|---|---|
| `fabrics` | fabric stash and shopping list |
| `fabric_pairings` | "goes well with" links, stored in both directions |
| `accessories` | bows, scarves, costumes; `pair_id` links a matching set (both ways) |
| `accessory_fabrics` | which fabrics each accessory uses, as main / middle / accent |
| `events` | holidays, notable days, seasons, outings (optional end date); holiday rows remember which file entry and year they came from |
| `post_plans` | a planned post for an event |
| `looks` | which pet wears which accessory in a post |

The database enforces the rules itself: enum values and dates are checked,
colors and keywords must be JSON arrays, and you can't delete a fabric a bow
still uses, or an event or accessory that a planned post still uses.

## Running it

```bash
cargo test   # planner, holiday and database tests (database tests use in-memory SQLite)
cargo run    # sync holidays, show the next 60 days, and print the Halloween example's schedule
```

## Roadmap

- [x] **M1** — Domain models and the backward-planning engine, with tests
- [x] **M2** — SQLite storage with migrations
- [x] **M3** — Editable list of holidays & notable days (fixed dates, "4th Thursday of November", Easter, multi-day events like Shark Week)
- [ ] **M4** — Web app (Axum) with a calendar view and forms for inventory and events
- [ ] **M5** — Suggestions: "for Earth Day, consider the leaf-print bow" based on keywords and colors

## Project layout

```
data/
  holidays.toml  the editable holidays list
migrations/   numbered SQL scripts that build the schema
src/
  lib.rs      crate root
  models.rs   fabric, accessories, events, post plans
  planner.rs  the scheduling engine + its tests
  holidays.rs reads holidays.toml and calculates dates (Easter, nth weekday...) + tests
  db.rs       SQLite repository: saves and loads the models + its tests
  main.rs     demo binary (becomes the web server in M4)
build.rs      rebuilds when a migration changes
```
