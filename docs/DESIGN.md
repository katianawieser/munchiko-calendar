# Design notes

The decisions behind Munchiko Calendar, the alternatives considered, and what
each choice costs. For what the app does, see the [README](../README.md).

## The problem, and the shape that follows from it

Every post for the account needs prep with real lead times: fabric has to be
bought before a bow can be sewn, everything has to be ready before the photo
shoot, and the photo needs editing before it's published. Missing a holiday
means waiting a year. So the core question is *"given this post date, what
has to happen when, and are we already late?"*, and secondly *"what do we
already own that fits this event?"*

That gives the app two pure engines (a **planner** and a **suggester**) with a
thin storage layer and a thin web layer around them:

```
           ┌──────────────── web (Axum + Askama) ────────────────┐
           │ pages, forms, uploads, request tests                │
           └──────┬───────────────────────────────┬──────────────┘
                  │                               │
        ┌─────────▼─────────┐          ┌──────────▼──────────┐
        │ db (SQLite, sqlx) │          │ photos (files, sips) │
        └─────────┬─────────┘          └──────────┬──────────┘
                  │       pure core: no I/O       │
   ┌──────────────▼───────────────────────────────▼───────────────┐
   │ models · planner · suggest · holidays · palette · process     │
   └───────────────────────────────────────────────────────────────┘
```

**Decision: keep the core free of databases, files and HTTP.** `models`,
`planner`, `suggest`, `holidays`, `palette` and `photos::process` take values
and return values. That makes most of the logic testable with plain unit
tests and no setup, and it kept later changes local. Swapping the scoring
rules, or adding plural matching, never touched SQL or HTML.

## Planning

- **Work backward from the post date** over parallel tracks (each bow, each
  bought item, the backdrop). The track that must start earliest is the
  critical path; **slack** is the days between today and that start, and
  negative slack means the post is already behind.
- **Lead times are data, not code** (`LeadTimes`, plus a per-bow override),
  so they can change without touching the algorithm.
- **"When would I have to start a new bow from this fabric?"** is answered by
  running the real planner on a hypothetical bow instead of repeating its
  arithmetic in the suggester, so the two can never disagree.
- A post whose date has passed counts as finished, never "overdue". Late
  warnings only make sense for work that can still happen.
- **Ideas (posts with no date)** reuse the same planner: the idea is planned
  against a made-up date, and the schedule's length gives "needs 7 days of
  prep, so it could post on Oct 26 if prep starts today". There's no second
  set of rules to drift out of step.
- **Series (several posts from one setup and shoot)** reuse it too: the
  series' posts are merged into one hypothetical post (everyone's looks, the
  shared backdrop, the first post's date), planned as usual, and then its single
  publish task is swapped for one per post. The shoot lands before the
  *first* post; later posts are edited while they wait. The calendar draws each
  series once, so one backdrop and one shoot, not three.

## Suggestions: explainable scoring, not a model

Each event word scores once: 3 points if it matches a keyword (an
accessory's name or its fabrics' keywords), otherwise 2 if it matches a
color. Small bonuses (half of a matching set, ready to wear) apply **only
after a real match**, so a ready bow never gets suggested just for being ready.

- **Why not machine learning?** There's no training data, and the user needs
  to trust the result. Every suggestion carries its reasons ("autumn, orange
  (color) · matching set · ready to wear"), so a bad suggestion is easy to
  understand and fix by editing a keyword.
- **Plurals** ("Fall Leaves" vs a "leaf" fabric) are handled by a small
  rule-based `singular()` applied to *both* sides. It only needs to be
  consistent, not linguistically perfect. A Porter stemmer was rejected: it
  produces non-words ("christma", "berri") that would show up in reasons. A
  short exception list stops false matches like "news" → "new" (New Year).
- Ties sort by name, so the order never jumps around between page loads.

## Holidays: an editable file, not an API

About 45 holidays live in `data/holidays.toml` with date rules: fixed dates,
"4th Thursday of November", Easter offsets (Easter is calculated), per-year
dates for things like Shark Week, multi-day spans and offsets (Black Friday =
Thanksgiving + 1).

- **Why not a holiday API?** The interesting days for this account ("Ides of
  March", the pets' birthdays) aren't in public APIs, and an offline file
  can't break or rate-limit.
- On every start the file is synced into the database for this year and next:
  added, updated in place, or removed (unless a post is already planned for
  it). A holiday's **name is its identity** across syncs.
- The file is the source of truth, so holiday events are **read-only in the
  UI** (the repository refuses edits with a dedicated error, shown as a 403
  page that says to edit the file).

## Storage: SQLite with migrations, and rules in the schema

- **SQLite** fits a single-user local app: one file, no server, easy to back
  up. **Numbered migrations** (`migrations/0001…0005`) run on startup.
- **The database enforces rules itself:** `STRICT` tables (no text in integer
  columns), `CHECK` constraints on enums, dates and JSON shape, and foreign
  keys. The app checks too, but a bug can't quietly write bad data.
- **`ON DELETE NO ACTION`, not `RESTRICT`:** both block deleting a fabric a bow
  still uses, but SQLite reports `RESTRICT` with an error code sqlx doesn't
  map to a foreign-key violation, so the app couldn't tell "in use" from
  "broken".
- **Short word lists (colors, keywords) are JSON arrays** in one column, not
  join tables: they're always read and written whole, and never queried
  on their own.
- **Runtime-checked queries** (`sqlx::query`, not the `query!` macro): the
  macro needs a live database at compile time, which complicates a fresh
  clone and CI. The trade-off is that SQL typos surface in tests, not at
  compile time; every repository method is covered by an in-memory test.
- **Rebuilding a table that other tables point at:** making a post's date and
  event optional meant rebuilding `post_plans` (SQLite can't drop NOT NULL in
  place). Dropping the old table would cascade-delete every look and post
  photo. SQLite's recipe switches foreign keys off first, but that can't
  happen inside a transaction, and sqlx always wraps SQLite migrations in one
  (it ignores `-- no-transaction` for SQLite, found by testing). So migration
  0006 saves the child rows, removes them, rebuilds, and puts them back, all in
  one transaction. A test stops a database at migration 5, adds data the old
  way, then runs 0006. On the real data's backup, a stale build ran the first
  version of this migration: it failed, and the transaction left everything
  untouched.
- **Changing stored data safely:** when colors went from plain words to
  `{name, hex}` objects, migration 0004 rewrote existing rows with SQLite's
  JSON functions, keeping their order. A test writes an old-format row and runs
  the migration's SQL on it, because a fresh test database has nothing to convert.

## Web: server-rendered pages with one optional script

- **Askama templates compiled into the binary** (a template typo is a build
  error) and plain HTML forms, styled with Pico CSS. No front-end framework:
  for forms and lists it would add a build step and a second codebase for
  little gain.
- **Post/Redirect/Get** for every form (refreshing never resubmits), **422**
  with the user's input kept on validation errors, **409** for "still in use".
- **Guided errors:** a blocked delete shows the chain of what's in the way
  (fabric → bow → post), with links and a de-duplicated "start here", instead of
  a dead-end message.
- **Security for a localhost app:** it listens on 127.0.0.1 only, with no login.
  It still has **cross-site request protection**, because any website you
  visit can make your browser post to localhost: requests whose
  `Sec-Fetch-Site` (or `Origin`) isn't our own page are refused. Redirect
  targets are checked (no open redirects), and color values are validated as
  `#rrggbb` before they go into a `style` attribute.
- **Progressive enhancement:** the only script, `colors.js`, adds swatch
  pickers and color chips; the forms work without it. One subtlety: a browser
  color input can't be blank (it always holds a color), so the picked shade is
  copied into a hidden field that stays empty until a shade is really chosen.

## Photos

- **Files on disk, records in the database.** Multi-megabyte images would
  bloat the database and its backups; files can be served and removed
  directly. Files are named `<id>.<ext>`, never by the uploaded name, so an
  upload can't write elsewhere (`../../munchiko.db`).
- **"A photo belongs to exactly one thing", enforced by the schema:** one
  nullable foreign key per owner type (fabric, accessory, post plan) plus a
  `CHECK` that exactly one is set. The common alternative (`owner_type` +
  `owner_id`) can't use foreign keys, so the database couldn't guarantee the
  owner exists or clean up when it's deleted.
- **`AUTOINCREMENT` so ids are never reused.** Photos are cached as immutable
  by the browser; a reused id would show an old photo for a new one.
- **Uploads are checked by content, not name:** the first bytes ("magic
  number") decide the type, and every image is fully decoded to make its
  thumbnail, which also rejects damaged or fake files. A multi-file upload
  is all-or-nothing.
- **Keeping database and disk in step:** the record is written first (it
  assigns the id), then the files; if a file write fails, the record is
  removed again. Deleting an owner lists its photos first, then removes the
  files after the database delete succeeds.
- **Bigger request limits only where needed:** upload routes accept 60 MB
  (15 MB per photo); every other route keeps the 2 MB default.

### Image processing

- **The `image` crate (pure Rust)** for thumbnails and BMP/TIFF → PNG, so
  there's nothing to install. CPU-heavy work runs in `spawn_blocking` so it
  doesn't stall other requests.
- **Defensive decoding:** images over 16,384 px on a side are refused before
  any memory is used (a tiny file can claim to be enormous). Phone photos are
  turned upright using their EXIF orientation, so thumbnails match what the
  browser shows. Transparency is blended onto white for JPEG thumbnails, and
  small images are never enlarged (a test caught the library doing that).
- **HEIC (iPhone) via macOS's built-in `sips` tool.** There is no dependable
  pure-Rust HEIC decoder (HEVC is complex and patent-encumbered), and the C
  library would make every contributor install it. The converter is chosen
  once at startup, so other systems (and the tests) fall back to an "export as
  JPEG" message. The process gets fixed arguments plus our own temp paths,
  a timeout, and kill-on-drop. Testing found that `sips` exits successfully
  even when it skips an unreadable file, so a missing output file counts as
  failure, and the output is checked to really be a JPEG.

### Colors from photos

- **Buckets, not k-means:** pixels are sorted into 4,096 color buckets, and
  look-alike buckets are merged biggest-first. K-means starts from random
  guesses, so the same photo could give different answers; buckets are
  deterministic, explainable and testable.
- **"Look alike" is measured in CIE Lab**, a color space where distance tracks
  how different colors look to people (RGB distance doesn't). The conversion
  is tested against published values.
- Each color gets the **nearest name** from a built-in list, because names
  are what suggestions match on. The simple distance formula over-weights
  saturation, so a name can have two reference shades (two "navy"s) rather
  than switching to the much more complex CIEDE2000 formula.
- **Computed from thumbnails on each page load:** fast enough that nothing
  needs storing or keeping in sync.

### Warm and cool

- **One shade:** neutral if it has almost no color (Lab chroma under 15:
  white, cream, greys, black); otherwise warm from pink through red and
  orange to yellow (hue 330°–75°), cool for the rest of the wheel. Browns and
  tans count as warm. Colorfulness uses Lab because in plain RGB a pale cream
  looks "very saturated".
- **A whole item:** a tone wins when it has more than twice as many colors as
  the other; otherwise it's "warm + cool". My first rule (any cool color makes
  it mixed) put half of my real bows in "mixed", because one navy accent was
  enough, so the filter wasn't useful. Neutrals don't count either way.
- **Computed, never stored:** tone is derived from the swatches on each page
  load, so it can't go stale when a color is edited, and there's no migration.
- **Filters live in the address** (`?tone=warm&sort=color`), so a filtered list
  can be bookmarked and the status buttons return to it. Table vs grid is a
  standing preference, so it's also remembered in a cookie.

## Performance: measure first

Pages with photos first took ~0.9 s in a debug build. Timing showed a
thumbnail alone took 3 ms, so the cost was in the color analysis. Three
changes:

1. Counting pixels in a fixed array instead of a `HashMap`.
2. Keeping each group's Lab value instead of recalculating it on every comparison.
3. Compiling dependencies optimized even in debug builds (`[profile.dev.package."*"]`).

Result: ~0.14 s under `cargo run`, ~0.02 s in a release build, with the same
output (the existing tests guarded the refactor).

## Testing

- **Unit tests** on the pure modules: planner dates, holiday rules (Easter,
  "last Monday"), scoring and reasons, plurals, Lab values, color merging,
  magic bytes, thumbnails.
- **Repository tests** on a fresh **in-memory SQLite** database each, including
  foreign-key and cascade behavior and the data migration.
- **Request tests** build the whole router and send requests directly (no
  network), checking status codes, redirects and HTML, including real
  multipart uploads. "Today" is pinned (`today_override`) so results never
  depend on the date; photo tests use temporary folders.
- **Platform-dependent tests** (the HEIC conversion) skip themselves where
  `sips` isn't available, and a separate test covers the fallback.
- **Beyond `cargo test`:** schema changes were tried on a *backup copy* of the
  real database before release, and the one script's chip behavior was
  checked in headless Chrome.

## Known limitations, and what I'd do next

- **Single user, localhost only.** Sharing it would need authentication and
  per-user data; the repository layer is the natural seam for that.
- **HEIC conversion needs macOS**; AVIF isn't accepted (decoding it needs a C
  library, the same trade-off as HEIC).
- **Color names are approximate** (a salmon-pink can come out as "peach"), and
  a photo's background can show up as a suggested color. Both are one click
  to ignore or rename.
- **Next:** photos in the suggestion box; a calendar nudge for upcoming events
  with no post yet.
