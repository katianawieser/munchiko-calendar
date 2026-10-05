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
| `events` | holidays, notable days, seasons, outings (optional end date) |
| `post_plans` | a planned post for an event |
| `looks` | which pet wears which accessory in a post |

The database enforces the rules itself: enum values and dates are checked,
colors and keywords must be JSON arrays, and you can't delete a fabric a bow
still uses, or an event or accessory that a planned post still uses.

## Running it

```bash
cargo test   # planner and database tests (the database tests use in-memory SQLite)
cargo run    # save the Halloween example to munchiko.db on first run, then print its schedule
```

## Roadmap

- [x] **M1** — Domain models and the backward-planning engine, with tests
- [x] **M2** — SQLite storage with migrations
- [ ] **M3** — Editable list of holidays & notable days (fixed dates, "4th Thursday of November", Easter, multi-day events like Shark Week)
- [ ] **M4** — Web app (Axum) with a calendar view and forms for inventory and events
- [ ] **M5** — Suggestions: "for Earth Day, consider the leaf-print bow" based on keywords and colors

## Project layout

```
migrations/   numbered SQL scripts that build the schema
src/
  lib.rs      crate root
  models.rs   fabric, accessories, events, post plans
  planner.rs  the scheduling engine + its tests
  db.rs       SQLite repository: saves and loads the models + its tests
  main.rs     demo binary (becomes the web server in M4)
build.rs      rebuilds when a migration changes
```
