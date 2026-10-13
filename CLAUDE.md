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
- **M4c done (M4 complete):** events list/detail/new/edit/delete (`events.rs`);
  `Event.from_holidays_file` (from `holiday_key`), and `update_event`/`delete_event`
  refuse holidays with `DbError::ReadOnly` → 403 page. Post plan new/edit/delete
  (`/posts/new?event=ID` pre-fills event + date; empty date = event day);
  `update_post_plan` replaces looks. Calendar event entries link to `/events/{id}`.
- **M5a done:** `src/suggest.rs` (pure, 8 tests). Event words = name + keywords,
  lowercased, minus filler ("day", "national", "bow"...). Accessory keywords = name
  words + its fabrics' keywords; colors = own + fabrics'. Per event word: keyword 3,
  else color 2; bonuses only after a match: matching set +1, ready +1. Ties sorted by
  name. `suggest_fabrics` adds `start_by` by running the planner on a hypothetical new
  bow (no duplicated date logic). No stemming (leaf ≠ leaves) — known limitation.
- **Tweaks after first real use (done):** (1) accessories have `colors_from_fabrics`
  (migration 0003, default 0 for old rows, on for new ones); `Accessory::effective_colors`
  = fabrics' colors (main first) + own extra colors, used by lists and `suggest`.
  (2) Blocked deletes: `web/blockers.rs` (pure) builds the chain fabric → accessory →
  post with links and a de-duplicated "Start here"; edit pages show it instead of the
  Delete button (`templates/delete_section.html`); server still returns 409.
  (3) Bug fix: example data is seeded only when `Repository::has_user_data()` is false
  (it used to re-seed whenever there were no posts). Katiana now has real data in
  `munchiko.db` — test schema changes on a `.backup` copy, never the real file.
- **Next: M5b — show suggestions** on the event page ("Ideas for Halloween") and above
  the post form's checkboxes (not pre-ticked); fabric ideas + "start by" when no
  accessory fits. Possible polish: drop "(bow)" from planner task titles.

## Original notes (summary of my handwritten outline)
Planner looks ahead for upcoming events/holidays/notable days; knows bows made, inventory
and fabric (including to-be-made); knows planned events/outings; suggests "for X day,
consider Y bow", when to make the bow, and when to take photos. Parts: calendar
interface, bow inventory, fabric inventory, event list (past + future). I can input new
events/outings (and get planning advice), new bows (linked to fabric) and new fabric.
Fields — Fabric: name, description, colors, keywords, pairings. Bow: name, description,
colors, fabrics (main, middle), made/bought (brand), pairing. Scarf: same, fabrics and
pairing can be empty. Event: start/end date, name, description, type, keywords.
