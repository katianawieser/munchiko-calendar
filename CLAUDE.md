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
  bow (no duplicated date logic).
- **Tweaks after first real use (done):** (1) accessories have `colors_from_fabrics`
  (migration 0003, default 0 for old rows, on for new ones); `Accessory::effective_colors`
  = fabrics' colors (main first) + own extra colors, used by lists and `suggest`.
  (2) Blocked deletes: `web/blockers.rs` (pure) builds the chain fabric → accessory →
  post with links and a de-duplicated "Start here"; edit pages show it instead of the
  Delete button (`templates/delete_section.html`); server still returns 409.
  (3) Bug fix: example data is seeded only when `Repository::has_user_data()` is false
  (it used to re-seed whenever there were no posts). Katiana now has real data in
  `munchiko.db` — test schema changes on a `.backup` copy, never the real file.
- **M5b done (M5 complete; original roadmap done):** `web/ideas.rs` (pure, tested)
  picks top 5 accessory ideas with readable reasons (keywords, colors, then bonuses);
  only if none fit, top 3 fabrics with "start by" and a too-late flag. Shown on the
  event page and above the post form's checkboxes (never pre-ticked; reflects the
  event the form was opened/submitted with, since there's no JS).
- **M6a done:** `models::Color { name, hex: Option<String> }` (hex validated by
  `Color::parse_hex` → lowercase `#rrggbb`, so it's safe in a style attribute).
  Migration 0004 rewrites both `colors` columns from `["x"]` to `[{"name":"x"}]`
  (order kept with `json_group_array(... ORDER BY key)`; tested in db.rs). Names stay
  required and are all `suggest` looks at. `effective_colors` dedupes by name, first
  wins (so a fabric's swatch beats an accessory's bare extra). Forms send repeated
  `color_name` + hidden `color_hex` pairs, matched by position (`forms::read_colors`).
  First JS in the app: `static/colors.js` (progressive enhancement: pickers,
  add/remove; the hidden field stays empty until a shade is picked, since a color input
  can't be blank). Partials `color_fields.html`, `color_chip.html`. Katiana's existing
  accessories all have `colors_from_fabrics` off, so they only show fabric swatches
  once she turns that switch on.
- **M6b done (M6 complete):** migration 0005 `photos` table: one nullable FK per owner
  (fabric/accessory/post plan, ON DELETE CASCADE) + CHECK exactly one, AUTOINCREMENT so
  ids (and cached `/photos/{id}` URLs) are never reused. `src/photos.rs` (pure-ish, no
  db): `ImageType::detect` by magic bytes (HEIC refused with an export hint),
  `PhotoStore` writes `<id>.<ext>` in `PHOTOS_DIR` (default `photos/`, gitignored).
  `web/photos.rs`: multipart upload (axum `multipart` feature; per-route
  `DefaultBodyLimit` 60 MB, 15 MB per photo), all files validated before any is saved,
  record first then file (record removed if the write fails); serve with nosniff +
  immutable cache; owner deletes list photos first, then remove files after the delete.
  `AppError::PhotoRejected` → error page with a back link. Upload is a separate form
  above the main form on edit pages (forms can't nest; keeps the photo on screen for
  the eyedropper); new forms say "add photos once saved". Post page has a Photos
  section. Full-size images are used as thumbnails (no resizing yet).
  Tests: `setup_with_photos` gives a temp folder (tempfile dev-dep).
- **M6c done:** `image` 0.25 crate (no default features; jpeg/png/webp/gif/bmp/tiff,
  MSRV 1.85). `src/photos/` is now a folder: `mod.rs` (`detect` → `Upload::Ready(ImageType)`
  or `Upload::Convert(Foreign::{Bmp,Tiff,Heic})`; DB still stores only the 4 browser
  types), `process.rs` (pure: `decode` with 16384px limits + EXIF orientation,
  `thumbnail` → JPEG ≤400px, never enlarges, alpha blended onto white; `to_png`),
  `heic.rs` (`HeicConverter::{Unavailable, Sips}`; `system()` picks sips on macOS; temp
  dir, fixed args, 60s timeout, kill_on_drop; sips exits 0 even when it skips a bad
  file, so a missing output file = failure, and output is checked to be JPEG).
  `AppState.heic` defaults to Unavailable; `main` sets `system()` and prints it.
  Upload decodes every file (proves it's a real image), runs image work in
  `spawn_blocking`, saves `<id>.thumb.jpg` beside the photo. `/photos/{id}/thumb` makes
  missing thumbnails on first view (photos from M6b) and keeps them; falls back to the
  original if it can't. Lists and galleries use thumbnails; click opens the original.
  AVIF not accepted (decoding needs a C library).
- **M6d done (M6 complete):** `src/palette.rs` (pure): pixels → 4,096 buckets (array,
  4 bits/channel) → merge biggest-first by CIE Lab ΔE76 < 15 (Lab cached per group) →
  keep ≥4% share, ≤6, one per name. Deterministic (no k-means). `NAMES` ≈46 sewing
  names; a name may appear twice (two "navy" shades, since ΔE76 over-weights
  saturation). Computed per page load from thumbnails (`web/photos.rs::color_ideas`,
  `thumbnail_of` shared with the thumb route); ideas whose name already has a swatch are
  hidden. Accessory compares with `effective_colors`. colors.js chip click fills:
  last-focused row (keeps its name) > same-name row > empty row > new row; tested in
  real Chrome via a `--dump-dom` harness. `[profile.dev.package."*"] opt-level = 2`
  so image decoding is fast under `cargo run`. Page with photos: ~0.14 s debug,
  ~0.02 s release. Names are approximate (pink photo → "peach"); user edits them.
- **M7a done (plural matching):** `suggest::singular` (pub, rule-based, not Porter:
  IRREGULAR table, NOT_PLURAL list e.g. christmas/news/canvas, ≤3 letters unchanged,
  -ies→-y if stem ≥2, -ches/-shes/-sses/-xes drop "es", -ss/-us/-is kept, else drop
  "s"). Both sides normalized; `event_words` is a BTreeMap singular → as written, so
  reasons show the event's own word. Real data: 8 events changed, all from "reds"→red.
  No let-chains (MSRV 1.85; they need 1.88).
- **M7b done (portfolio polish):** README restructured for a portfolio reader
  (pitch, highlights, built-with, quick start, 6 captioned screenshots, then reference
  sections); `docs/DESIGN.md` = decisions + alternatives + trade-offs, testing,
  limitations (written for interviewers). Screenshots: 2x, Katiana's real data on a
  scratch copy, plus two posts planned only for the screenshots (Thanksgiving: Orange
  Sunflower + Yellow Wheat; Christmas: Candy Canes) and Pokemon Grid's swatches reset
  on the copy so the photo-color chips show (trimmed view, disclosed in README).
  Tagged `v1.0`. Push to GitHub only after Katiana has looked (her choice).
- **Possible next steps (not started; ask first):** photo thumbnails in the ideas box; drop "(bow)" from planner task
  titles; "upcoming events with no post yet" nudge on
  the calendar; deploying/screenshots for the portfolio.

## Original notes (summary of my handwritten outline)
Planner looks ahead for upcoming events/holidays/notable days; knows bows made, inventory
and fabric (including to-be-made); knows planned events/outings; suggests "for X day,
consider Y bow", when to make the bow, and when to take photos. Parts: calendar
interface, bow inventory, fabric inventory, event list (past + future). I can input new
events/outings (and get planning advice), new bows (linked to fabric) and new fabric.
Fields — Fabric: name, description, colors, keywords, pairings. Bow: name, description,
colors, fabrics (main, middle), made/bought (brand), pairing. Scarf: same, fabrics and
pairing can be empty. Event: start/end date, name, description, type, keywords.
