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

## Using the app

| Page | What you can do |
|---|---|
| **Calendar** | Month view of holidays, outings and every post's prep tasks; "to do today" box; overdue work outlined in red |
| **Posts** | Plan a post for an event, choose who wears what, see its prep timeline and slack |
| **Events** | Upcoming / past holidays and outings; "Plan a post" from any event; add and edit your own outings (holidays are edited in `data/holidays.toml`) |
| **Accessories** | Bows, scarves, costumes: filter, one-click status, fabrics by role, matching sets |
| **Fabrics** | Stash and shopping list, pairings, what each fabric is used by |

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

On startup the app runs any new database migrations, syncs holidays from
`data/holidays.toml`, and on the very first run adds the Halloween example
post so there's something to look at. Settings (all optional):
`DATABASE_URL` (default `sqlite:munchiko.db`) and `ADDRESS` (default
`127.0.0.1:3000`; only this computer can reach it, and there's no login).

```bash
cargo test   # planner, holiday, database and web tests (all on in-memory SQLite)
cargo run    # start the web app at http://127.0.0.1:3000
```

## Roadmap

- [x] **M1** — Domain models and the backward-planning engine, with tests
- [x] **M2** — SQLite storage with migrations
- [x] **M3** — Editable list of holidays & notable days (fixed dates, "4th Thursday of November", Easter, multi-day events like Shark Week)
- [x] **M4** — Web app (Axum) with a calendar view and forms for inventory and events
  - [x] **M4a** — server, month calendar, "to do today" box, post list and timeline pages
  - [x] **M4b** — fabric and accessory pages and forms
  - [x] **M4c** — event and post plan forms, edits and deletes
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
  suggest.rs  scores accessories and fabrics for an event, with reasons + tests
  holidays.rs reads holidays.toml and calculates dates (Easter, nth weekday...) + tests
  db.rs       SQLite repository: saves and loads the models + its tests
  demo.rs     the example Halloween post
  web/        the Axum web app
    mod.rs      router, shared state, error pages
    month.rs    month-grid layout and "to do today" (pure logic + tests)
    forms.rs    reading form submissions + choices for selects
    calendar.rs, posts.rs, events.rs, fabrics.rs, accessories.rs   page handlers
    tests.rs    request-level tests against an in-memory app
  main.rs     starts the server
templates/    Askama HTML templates
static/       Pico CSS (v2.1.1, MIT) and the app's stylesheet
build.rs      rebuilds when a migration changes
```
