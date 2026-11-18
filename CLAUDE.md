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
- **M8 (flexible posts), planned with Katiana:** M8a ideas (no date, optional
  event, title), then M8b series (several posts from ONE shared setup + shoot, each
  published on its own day; prep and shoot planned once before the first post).
  Gap finder: she chose to skip it for now.
- **M8a done:** migration 0006 rebuilds post_plans (title, nullable event_id and
  post_date, CHECK title or event). sqlx 0.8.6 SQLite ignores `-- no-transaction`, so it
  saves looks + post photos to temp tables, deletes them, rebuilds, and re-inserts
  (tested from a version-5 database). `PostPlan::{is_idea, name(event)}`. Planner:
  `build_timeline` → `PlanError::NotScheduled` for ideas; `build_timeline_on(plan, date)`;
  `readiness()` plans against 2000-01-01 → prep items (longest first) + `days_needed`;
  shoot title names only the pets in the looks. Web: `LoadedPost { event: Option, name,
  readiness }`; calendar skips ideas; Posts page has an Ideas section (ready first);
  idea page has prep table + "Schedule it" (POST /posts/{id}/schedule); form has
  title, optional event (incl. "Past:" events), When radio (date / idea);
  `/posts/new?idea=1`. Katiana's real db goes to version 6 on next start (tested on a
  copy: 8 photos kept, foreign_key_check clean).
- **Tweak (after M8a):** accessory form shows "From its fabrics: ● …" under the
  "Use colors from these fabrics" switch (server-rendered from the selected fabrics;
  colors.js updates it live from hidden per-fabric `<template class="fabric-chip-set">`
  chips, deduped by name in role order, hidden when the switch is off). Shared
  `models::without_repeats` is used by `effective_colors` and the form.
- **M8b done (M8 complete):** UI name "series"; start from a post ("+ Another post from
  this shoot" → POST /posts/{id}/series, `Repository::series_from_post` moves the
  backdrop) or "+ New series" (/series/new). Migration 0007: `series` table +
  `post_plans.series_id` (ON DELETE SET NULL; `delete_series` copies the backdrop
  back to posts with none). Planner: `combined_plan` (looks union, series backdrop,
  earliest date) + `build_series_timeline` (plans combined, swaps Publish for one
  "Publish: {post}" per dated post; Timeline.post_date = LAST post). `load_posts`
  plans each series once (`SharedPlan`, `SeriesInfo { posts, first_post }`); undated
  members of a scheduled series show "Photographed at the series' shoot" from
  first_post. Calendar draws a series once (PlannedPost.series → /series/{id}; publish
  labels from task titles). Health "Published" uses the post's own date; task rows
  are "Done" once the whole timeline is over (`posts::task_rows`, shared with
  series.rs). Post form refactored: `FormOptions` + `NewPost` start. Askama escapes
  `&` as `&#38;` (matters in tests); calendar entries repeat labels in `title=`.
- **Real-use notes (2026-10-27), planned with Katiana:** M9 smoother entry → M10
  organizing → M11 accessory types, check in after each. Her answers: note 1 meant a
  reusable palette of her own picked shades; batik = a tag (keyword); quantity is
  shown AND lets both pets wear the same bow in one post (warn if a post needs more
  than owned). Plan: M9a photos in the main form; M9b suggested color name for a
  custom-picked shade (nearest `palette::NAMES`), "✓ shade set" feedback, "Your shades"
  chips (derived from existing fabric/accessory colors, no new table); M9c "+ New
  fabric…" in the accessory form's fabric selects (created on Save, not in stock).
  M10: grid view for fabrics/accessories, sort/filter by warm/cool/neutral tone (from
  swatches), accessory tags (used by suggest). M11: kinds hat + prop, quantity,
  outfits (group pieces like Glinda's crown/dress/wand; choosing it in a look adds all).
  Data then: 41 fabrics, 21 accessories, 59 photos.
- **M9a done:** `web/photos.rs`: `FormWithPhotos` extractor (multipart or plain form →
  `FormData` + photos; back link = edit page or `/new`), `prepare_all` →
  `PhotoProblem { status, message }` (413 too big / 422), `save_all`, `with_photos`
  merges form + photo errors and says "choose your photos again" (browsers can't
  refill file inputs). Fabric/accessory forms are multipart with `photo_input.html`
  inside; their create/update routes get the 60 MB body limit via `.layer` on the
  MethodRouter. `PhotoSection::gallery` (no upload form) vs `::new` (post page, upload
  form with `auto-upload`). `static/photos.js`: object-URL previews (HEIC/TIFF show the
  file name), auto-submit on post pages. Old `/…/{id}/photos` routes kept (tests use
  them). macOS has no `timeout` command; zsh errors on unmatched globs in `rm`.
- **M9b done:** `GET /colors/name?hex=` (`web/colors.rs`, text/plain, 400 on bad hex)
  so naming stays in Rust (`palette::nearest_name`), not duplicated in JS.
  `palette::{rgb_from_hex, rainbow_order (hue +15° so bluish reds stay with reds;
  greys last), look_alike (ΔE < 5)}`. `colors::your_shades(fabrics, accessories)`:
  every named shade with a hex, same name + look-alike merged, rainbow order;
  added to every fabric/accessory form render via async `form_response(&state, …)`.
  colors.js: picker "change" → "✓ shade set" (`.shade-status`, fades after 4 s) +
  `suggestName` (only fills empty or previously suggested names; typing clears
  `data-suggested`); chips handled for `.color-ideas` and `.your-shades`
  (`<details>`). Katiana's data: 186 shades → 176 after merging (75 names), so the
  shades really differ. Pico gotcha: a `<small>` right after an input becomes a
  full-width help line; use `<span>` inside flex rows. Browser tests needing
  fetch from the file:// harness: Chrome `--disable-web-security` + throwaway
  `--user-data-dir` + `--virtual-time-budget`.
- **M9c done (M9 complete):** accessory fabric selects end with "+ New fabric…" (value
  `new`) and a `new_{field}` name box (colors.js shows it only for "new"; always shown
  without JS). `read_form` collects `new_fabrics: Vec<(FabricRole, String)>` ("Give the
  new main fabric a name." if empty); `fabrics_for_new_names` (before create/update)
  reuses a fabric with the same name ignoring case, else creates it with
  `in_stock: false` (Shopping list; "Bought it" moves it), one fabric for the same new
  name in two roles, then sorts fabric uses back into ROLES order (main first). Test
  gotcha: in Rust strings a trailing `\\` + newline is a literal backslash, not a
  continuation (Python heredocs make this easy to get wrong). Slow first runs after a
  rebuild are macOS XProtect scanning the new binary.
- **Katiana uses Firefox (on macOS) as her main browser.** Check browser-side
  changes in Firefox, not only Chrome: headless Firefox with `--marionette` and a
  throwaway `-profile`, driven by a ~50-line stdlib Python Marionette client
  (length-prefixed JSON over TCP 2828: `WebDriver:NewSession`, `Navigate`,
  `ExecuteAsyncScript`, `TakeScreenshot`); it loads the real app pages, so forms and
  same-origin fetches work. Verified in Firefox 158: color rows, name suggestion,
  Your shades, photo preview, live fabric colors, real multipart submit (CSRF passes),
  post-page auto-upload. Firefox/Safari on macOS use the system color panel, which can
  stay open, so colors.js also treats a shade as settled after 750 ms without
  changes (not only on "change"). Stop the test server with
  `lsof -ti tcp:3077 -sTCP:LISTEN`; without `-sTCP:LISTEN` it also kills the browser.
- **M10a done:** `palette::{Tone, tone}` (Lab chroma < 15 → Neutral, else HSL hue
  330°–75° Warm, else Cool; browns warm, magenta cool, hot pink warm). `web/browse.rs`
  (pure, tested): `item_tone` (picked hex, else a `palette::NAMES` name; a tone wins at
  more than 2× the other, else Mixed "Warm + cool"; neutrals ignored). The first rule
  (any cool → mixed) put 25/54 real accessories in Mixed, now 19 warm / 11 cool / 18
  mixed / 2 neutral. `Browse::read(query, headers)`: `tone`, `sort=color` (stable sort by
  `rainbow_order` of the first known shade, unknown last), `view` from the address else
  the `list_view` cookie (Set-Cookie only when the address chose). `Controls`/`Filter`
  feed the shared `templates/browse_controls.html` (page's own selects + tone + sort +
  Table | Grid links that keep the filters). Grid cards: square photo crop needs
  `overflow: hidden` (else tall photos stretch the aspect-ratio box). Accessory `back`
  keeps all params. **Firefox gotcha (2026-10-28):** when Firefox has a staged update
  (`~/Library/Caches/Mozilla/updates/.../update.status` = `applied`), a headless launch
  tries to install it, spawns `org.mozilla.updater`, and never opens Marionette; it may
  also have closed Katiana's open Firefox. Don't launch Firefox then; use headless
  Chrome for HTML/CSS-only checks, or ask her to restart Firefox first. Also: `pkill -f`
  with a pattern in the same command line kills its own shell.
- **M10b done (M10 complete):** migration 0008 `accessories.tags` (JSON array, default
  `[]`); `Accessory::all_tags(fabrics)` = own tags + fabrics' keywords (main first),
  deduped ignoring case; used by `suggest_accessories` (replacing name + fabric
  keywords) and the list. Form: "Tags" input (`split_list`, so lowercased). UI calls
  fabric keywords "Tags" too (column still `keywords`). `Browse.tag` (`?tag=`,
  compared by `tag_key` = lowercase + `singular`), `keeps(colors, tags)`,
  `controls(path, filters, params, tags)` adds the tag dropdown (all tags in use, not
  only those shown; a stale tag from a link stays selected), `encode` percent-encodes
  params. Real data: 14 batik fabrics → 3 batik accessories; 26 of 54 accessories have
  no fabrics linked, so they inherit nothing. Verified in Firefox 159 (after Katiana
  restarted it): Table→Grid click, square photos (54 cards), cookie carries grid to
  /fabrics, real filter-form submit keeps selects, status button returns to the
  filtered list. Turtle Batik Bow is "mixed": its pale baby blue (#b4d7e1) counts as
  neutral, leaving 4 cool : 2 warm, and exactly 2× isn't more than 2×.
- **M11 plan:** M11a kinds, M11b quantity (×N, both pets can share, warn when a post
  needs more than owned), M11c outfits; check in after each.
- **M11a done:** `AccessoryKind::{Hat (3 days), Prop (2 days)}`; KINDS has 5. Migration
  0009 rebuilds `accessories` (CHECK on kind can't be altered): saves
  accessory_fabrics / looks / accessory photos to temp tables (old rowid order), deletes
  them, creates `accessories_new` whose `pair_id REFERENCES accessories_new` (if it
  referenced `accessories`, dropping the old table would SET NULL every pair in the new
  one; RENAME rewrites the self-reference), single INSERT…SELECT (FKs checked at
  statement end), recreates `accessories_one_partner`, re-inserts children.
  `db::tests::database_at_version(n)` helper (used by the 0006 and 0009 tests). Real
  data copy 7→9: every row of accessories / accessory_fabrics / looks / photos
  identical, foreign_key_check + integrity_check clean. "hat"/"prop" are NOT filler
  words in suggest, so "National Hat Day" matches hats by name. Verified in Firefox 159.
- **M11b done:** migration 0010 `accessories.quantity` (INTEGER ≥ 1 CHECK, default 1;
  plain ALTER, no rebuild). A plain number, not one row per copy (Katiana's choice);
  for an unmade item it's how many will be made (prep time unchanged; `make_days`
  can be raised by hand). Looks already allowed both pets on one accessory (PK is
  post+pet+accessory). `PostPlan::shortages(accessories)` (pure): one copy per pet
  wearing it → `Shortage { pets, owned }`, Display "Mochi and Momo both wear X, but
  there's only 1." Warning only, saving is allowed; shown on the post page (link to
  `/accessories/{id}`, the edit page) and the Posts list (upcoming + ideas), never for
  published posts. Doesn't change readiness/"Ready". Form "How many" (empty = 1);
  ×N in accessory table/grid and post-form checkbox labels. Real-data copy 9→10:
  54 accessories all quantity 1, other columns identical, checks clean.
- **M11c plan (Katiana's answers, 2026-11-18):** a piece can be in several outfits
  (join table); picking an outfit in a post COPIES its pieces into the looks (can
  untick one per post; later outfit edits don't change planned posts; planner
  unchanged); outfits are full items (name, notes, pieces, photos; status derived:
  ready when every piece is); not in suggestions yet. Split: M11c1 outfit pages +
  data, M11c2 post form "add outfit" + grouped looks. Check in after each.
- **M11c1 done:** migration 0011: `outfits` (name, description) + `outfit_pieces`
  (PK outfit+accessory, both FKs ON DELETE CASCADE: deleting a piece takes it out of
  outfits; deleting an outfit never blocks). **Any future rebuild of `accessories`
  must save/restore outfit_pieces** (like 0009's trap). Photos table rebuilt for a
  4th owner (`outfit_id`, CHECK exactly one): ids kept, and the AUTOINCREMENT counter
  saved/restored via `sqlite_sequence` (DROP TABLE drops it). `models::Outfit`
  (`piece_items`, derived `status` = least-finished piece, empty = Idea,
  `ready_count`). `Repository::{create,get,list,update,delete}_outfit` (INSERT OR
  IGNORE drops repeated pieces). `web/outfits.rs`: /outfits list (thumb, piece links,
  "1 of 2 ready", colors from pieces), form at /outfits/{id} with pieces as checkboxes
  in `<details>` per kind (open if a piece is ticked, count in summary), photos in
  the form, delete always allowed. Nav has "Outfits". Accessory page: "Part of X
  outfit". Real-data copy 9→11: 119 photos identical, counter 123 kept, checks clean.
- **M11c2 done (M11 complete):** Katiana's addition: outfit edits don't change
  posts, but a piece ADDED to an outfit is offered on the post. Migration 0012
  `post_outfits` (post, pet, outfit; cascades from post and outfit) +
  `post_outfit_known` (pieces the choice knew; 3-column FK to post_outfits, cascade;
  accessory cascade). Future rebuilds of post_plans/accessories must save these too.
  `PostPlan.outfits: Vec<OutfitChoice { pet, outfit_id, known }>` (also on
  NewPostPlan; `impl From<PostPlan> for NewPostPlan`). Pure `models::choose_outfits`
  (already-chosen keep `known`; newly ticked remember the outfit's pieces and, with
  `add_pieces`, add them; unticked are forgotten, pieces stay) and
  `PostPlan::outfit_offers` (in outfit, not known, not worn by that pet). Post form:
  per-pet `{pet}_outfit` checkboxes above "Pieces"; `static/outfits.js` ticks pieces
  live and sets hidden `outfits_live=1` so the server doesn't re-add unticked ones
  (without JS the server adds them). Post page: looks grouped "X outfit (a, b)";
  offers with "Add it" / "Not for this post" → POST /posts/{id}/outfit-piece (only
  pieces really on offer; both record it as known); none for published posts.
  JS verified in headless Chrome (Firefox had a staged update: not launched).
  Real-data copy migrated to 12, checks clean.
- **Real-use notes #2 (2026-11-18), planned with Katiana:** M12 quicker use → M13
  colors → M14 long lists; check in after each part. Her answers: "Done" buttons on
  BOTH calendar tasks and post timelines (make → accessory Ready, buy fabric → in
  stock, order → Ready; backdrop/shoot/publish get none); "+ New fabric…" from a bow
  page asks "I have it / need to buy it" (default have it) and copies the bow's
  colors; shades: BOTH looser grouping in Your shades AND a merge page; WebP probably
  fine already, AVIF needed (sips can read it). "Used up" → Shopping list is by
  design (means "buy again"). Plan: M12a Done buttons, M12b new-fabric choice +
  colors, M12c back-to-top + grid card size + AVIF; M13a name suggestion always
  shown as a clickable hint (never overwrite typed names; chips fill empty rows
  first, don't silently replace a set swatch; clearer Your shades label), M13b photo
  colors before saving (server endpoint, palette stays in Rust), M13c looser
  grouping + /shades merge page; M14a type-to-search fabric/partner pickers, M14b
  fabric thumbnails in accessory list, M14c partner suggestions via fabric
  pairings. Deferred: naming consistency (her names follow colors + line + style +
  size, e.g. "Red & Orange Celebration Japanese Large/Small").
- **M12a done:** `web/done.rs` (pure): `for_task(PlanTask) -> Option<DoneButton>`:
  make → "✓ Made it", order → "✓ It arrived" (both POST /accessories/{id}/status
  status=ready), buy fabric → "✓ Bought it" (POST /fabrics/{id}/stock `in_stock=1`:
  sets, not toggles; without the field the list's button still flips). Partial
  `done_button.html` (needs `done` + `back`). Shown in the calendar's to-do box (now
  and next up; NOT in grid cells, which repeat a task per day) and on post/series
  timeline rows until the timeline is over. `forms::local_back(back, fallback)`:
  any app path (starts with one `/`, no `//`, no `\`), used by both routes.
  Katiana's real data has no scheduled upcoming posts right now, so no buttons
  show until she schedules one.
- **M12b done:** "+ New fabric…" box is now `.new-fabric` (name + "Need to buy it"
  checkbox `buy_new_{field}`, unticked = in stash) in a `.fabric-role` wrapper beside
  the select's label (no nested labels); colors.js hides/shows the whole box.
  `NewFabricName { role, name, in_stock }`; a newly created fabric copies the
  accessory's OWN color rows (`accessory.colors`, not effective colors); an existing
  name is reused unchanged (stock + colors untouched). Note: new accessories start
  with "colors from fabrics" on, so their own rows are often empty → nothing copied.
  Show/hide verified in headless Chrome (Firefox update still staged).
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
