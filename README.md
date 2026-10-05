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

## Running it

```bash
cargo test   # run the planner's unit tests
cargo run    # print the Halloween demo schedule
```

## Roadmap

- [x] **M1** — Domain models and the backward-planning engine, with tests
- [ ] **M2** — SQLite storage with migrations
- [ ] **M3** — Editable list of holidays & notable days (fixed dates, "4th Thursday of November", Easter, multi-day events like Shark Week)
- [ ] **M4** — Web app (Axum) with a calendar view and forms for inventory and events
- [ ] **M5** — Suggestions: "for Earth Day, consider the leaf-print bow" based on keywords and colors

## Project layout

```
src/
  lib.rs      crate root
  models.rs   fabric, accessories, events, post plans
  planner.rs  the scheduling engine + its tests
  main.rs     demo binary (becomes the web server in M4)
```
