# Project context for Claude

Munchiko Calendar is a Rust planner for Mochi & Momo's social media account (a dog
and a cat photographed in mostly handmade bows/costumes against holiday or
notable-day backdrops). It doesn't make content; it schedules prep by working
backward from a post date: buy fabric → make bow → build backdrop → photo shoot → publish.
See README.md for the planning rules, data model and roadmap.

## Who I am and how we work
- I'm Katiana, a software engineer moving toward product/project/program management.
  This project is a **portfolio demo for job interviews**, so I need to understand and
  explain every design choice.
- Build in **milestones**. After each one, explain what was built and why, in terms I
  could repeat in an interview. Don't race ahead to the next milestone without checking in.
- Keep code idiomatic and well-commented, with unit tests for logic.
- Commit at the end of each milestone with a clear message.

## Decisions already made
- Interface: **web app** — Axum backend serving a browser UI with a calendar view.
- Storage: **SQLite** with migrations (I have only basic SQL, so explain the schema and queries).
- Holidays: a **built-in, editable seed file** of holidays and notable days with date rules
  (fixed dates, "4th Thursday of November", Easter, multi-day like Shark Week). No external API.
- Bows, scarves and costumes are one `Accessory` type with a `kind`; each is a separate
  item, and `pair_id` links a matching Mochi & Momo set.
- `models` and `planner` stay free of database/web code so they remain easy to test.

## Status
- **M1 done and verified:** `src/models.rs`, `src/planner.rs` (7 tests, all passing,
  clippy clean), demo in `src/main.rs`. Needs Rust 1.85+ (2024 edition).
- **M2 done:** `migrations/0001_initial_schema.sql` (7 STRICT tables), `src/db.rs`
  (`Repository` over sqlx, 11 tests on in-memory SQLite), `main.rs` seeds/loads
  `munchiko.db`. Decisions: colors/keywords as JSON arrays; fabric pairings and
  accessory sets are two-way; `ON DELETE NO ACTION` (not RESTRICT, whose error code
  sqlx doesn't map to a foreign-key violation); runtime-checked queries, not `query!`.
  Full edit/update methods are deferred to M4 when the forms need them.
- **M3 done:** `data/holidays.toml` (~45 entries incl. Mochi's birthday 10-15 and
  adoption day 01-11, Momo's birthday 03-03 and adoption day 07-05), `src/holidays.rs`
  (rules: date / nth_weekday / easter_offset / by_year, plus days and offset_days;
  validated with clear errors), migration 0002 (`holiday_key` + `holiday_year`),
  `Repository::sync_holidays` (this year + next on startup; updates in place, deletes
  removed holidays unless a post is planned). Name is the holiday's identity.
  Built-in copy via `include_str!` is used only if the file is missing.
  Shark Week's `by_year` is empty until Discovery announces 2027 dates.
- **M4 decisions:** server-rendered Askama 0.14 templates (newer needs Rust 1.88) +
  plain HTML forms, no JS framework; Pico CSS vendored in `static/`; holiday events
  read-only in the UI ("edit holidays.toml"); localhost only, no auth. Split into
  M4a/b/c with a check-in and commit after each.
- **M4a done:** `src/web/` (router, `AppState` with `today_override` for tests,
  `AppError` → 404/500 pages), `month.rs` (pure grid + agenda, unit tested),
  calendar + posts list + post timeline pages, request tests in `web/tests.rs`,
  `demo.rs` seed. Posts whose date has passed count as finished (never "overdue").
- **M4b done:** fabric + accessory list/new/edit/delete pages, stock toggle, status
  buttons, accessory filters; `Repository::update_fabric`, `set_fabric_pairings`,
  `update_accessory`. Forms read raw pairs via `web/forms.rs` (repeated checkbox
  fields), Post/Redirect/Get, 422 on validation errors with input kept, 409 on in-use
  deletes. `reject_cross_site` middleware blocks cross-site POSTs (CSRF). Accessory
  form has one fabric per role (main/middle/accent).
- **Next: M4c — events (outings editable, holidays read-only) and post plan forms**
  (new from an event, choose looks per pet, edit, delete).
- Then M5 suggestions ("for Earth Day, consider the leaf bow"
  by matching keywords and colors).

## Original notes (summary of my handwritten outline)
Planner looks ahead for upcoming events/holidays/notable days; knows bows made, inventory
and fabric (including to-be-made); knows planned events/outings; suggests "for X day,
consider Y bow", when to make the bow, and when to take photos. Parts: calendar
interface, bow inventory, fabric inventory, event list (past + future). I can input new
events/outings (and get planning advice), new bows (linked to fabric) and new fabric.
Fields — Fabric: name, description, colors, keywords, pairings. Bow: name, description,
colors, fabrics (main, middle), made/bought (brand), pairing. Scarf: same, fabrics and
pairing can be empty. Event: start/end date, name, description, type, keywords.
