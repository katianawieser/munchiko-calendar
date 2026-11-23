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
   │ models · planner · suggest · holidays · palette · shades ·    │
   │ process                                                       │
   └───────────────────────────────────────────────────────────────┘
```

**Decision: keep the core free of databases, files and HTTP.** `models`,
`planner`, `suggest`, `holidays`, `palette`, `shades` and `photos::process`
take values and return values. That makes most of the logic testable with plain unit
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
- **Tasks aren't stored; "Done" changes the thing itself.** A "make this bow"
  task exists because the bow isn't ready. So the Done button on the calendar
  or a timeline marks the *bow* ready (or the fabric bought), and the planner
  simply stops producing the task. There's no to-do list to fall out of step
  with the inventory. "Bought it" *sets* in-stock rather than toggling it, so a
  double click or an old tab can't put the fabric back on the shopping list.
- **Outfits are copied into a post, not referenced.** Choosing "Glinda" ticks
  its crown, dress and wand in the post, and any piece can be unticked for that
  post. Editing the outfit later doesn't silently change planned posts. The
  post remembers which pieces the outfit had when chosen, so a piece *added* to
  the outfit later is offered once ("add the cape to Momo's look?") while a
  piece left out on purpose is never offered back. Storing only "Momo wears
  Glinda" couldn't tell those two cases apart.

## Suggestions: explainable scoring, not a model

Each event word scores once: 3 points if it matches a keyword (an
accessory's name, its tags or its fabrics' tags), otherwise 2 if it matches a
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
- **Matching partners, measured before built.** Before scoring which bow is
  another's Mochi & Momo partner, I checked what actually links the 12 real
  sets: 9 share a fabric, none are linked only through fabric pairings, and
  the other 3 have no fabrics recorded but share name words ("Embroidered
  Candy Canes - Small / - Large"). So the score uses shared fabric, paired
  fabric and shared name words (ignoring sizes like "small"), with same kind as
  a bonus. Run against the real sets, the true partner came first for 22 of
  24; the two misses have nothing in common to go on. Fabric links alone would
  have missed a quarter of the sets.

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
  up. **Numbered migrations** (`migrations/0001…0015`) run on startup.
Migrations that rebuild a table or move existing data are tested from an
older database version, and every one was run on a copy of the real database
first.
- **The database enforces rules itself:** `STRICT` tables (no text in integer
  columns), `CHECK` constraints on enums, dates and JSON shape, and foreign
  keys. The app checks too, but a bug can't quietly write bad data.
- **`ON DELETE NO ACTION`, not `RESTRICT`:** both block deleting a fabric a bow
  still uses, but SQLite reports `RESTRICT` with an error code sqlx doesn't
  map to a foreign-key violation, so the app couldn't tell "in use" from
  "broken".
- **Short word lists (keywords, tags) are JSON arrays** in one column, not
  join tables: they're always read and written whole, and never queried on
  their own. Colors started that way too, and moved to their own table once
  they needed an identity shared across items (see the palette, below).
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

### Changing a rule in the schema means rebuilding the table

Allowed values (kinds of accessory, statuses) are `CHECK` rules in the
schema, so bad data can't get in even through a bug. The cost shows when the
list grows: SQLite can't change a `CHECK` in place, so adding hats and props
(migration 0009) rebuilt the accessories table. Tables that point at it would
lose rows (cascade) or block the drop, so their rows are saved, removed and
put back, all in one transaction. The table also points at itself (matching
sets), so the new copy refers to itself by its temporary name; otherwise
dropping the old table would clear every set. A test builds a version-8
database and checks that sets, fabric order, looks and photos survive; I also
compared every row of a copy of my real data before and after. I kept the
`CHECK` anyway: new kinds are rare, and the rule protects the data every day.

## Web: server-rendered pages, small optional scripts

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
- **Logic in Rust, JavaScript for presentation.** Four small scripts make
  pages nicer (shade pickers and chips, photo previews, ticking an outfit's
  pieces, the back-to-top button), and every form still works without them.
  Anything that decides something stays in Rust with tests, behind a small
  endpoint: color names (`/colors/name`), the main colors of a photo chosen
  but not yet saved (`/colors/from-photos`) and partner ideas
  (`/accessories/partner-ideas`) all return server-rendered HTML that the
  script just inserts, so there's no second copy of the markup or the rules
  in JavaScript. Where a script changes what the server should do, it says so
  explicitly: `outfits.js` sets a hidden `outfits_live=1`, so the server knows
  the outfit's pieces are already ticked (and one may have been unticked on
  purpose) and doesn't add them back.
- **Long lists became name boxes, after checking names are unique.** With 75
  fabrics and 54 accessories, dropdowns were hard to use. Fabric and partner
  pickers are text boxes with the stash's names as suggestions (a plain HTML
  `<datalist>`), and the server resolves the name (ignoring capitals); a new
  fabric name makes a new fabric on save. That's only safe because every name
  in the real data turned out to be unique, which I checked first. A hint under
  the box ("✓ one of your fabrics" / "+ new fabric") stops a typo from quietly
  creating a fabric.
- One subtlety: a browser color input can't be blank (it always holds a
  color), so the picked shade is copied into a hidden field that stays empty
  until a shade is really chosen.

## Photos

- **Files on disk, records in the database.** Multi-megabyte images would
  bloat the database and its backups; files can be served and removed
  directly. Files are named `<id>.<ext>`, never by the uploaded name, so an
  upload can't write elsewhere (`../../munchiko.db`).
- **"A photo belongs to exactly one thing", enforced by the schema:** one
  nullable foreign key per owner type (fabric, accessory, outfit, post plan)
  plus a `CHECK` that exactly one is set. The common alternative (`owner_type` +
  `owner_id`) can't use foreign keys, so the database couldn't guarantee the
  owner exists or clean up when it's deleted.
- **`AUTOINCREMENT` so ids are never reused.** Photos are cached as immutable
  by the browser; a reused id would show an old photo for a new one. Adding
  outfits as a fourth owner meant rebuilding the table (the `CHECK` again),
  and dropping a table also drops its `AUTOINCREMENT` counter, so migration
  0011 saves the counter and puts it back. On the real data: 119 photos kept
  their ids, and the counter stayed at 123.
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
- Each color gets the **nearest name** from a list of named colors, because
  names are what suggestions match on (see "Color names" below).
- **Photo colors reuse the palette first:** a palette color that's practically
  the same (ΔE under 5) is offered as "yours"; a close one (under 10) is offered
  next to the new shade ("red · or new: burgundy"); otherwise it's new. A new
  color never gets a name the palette already uses, or saving it would
  silently become that other color.
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
- **Tags are inherited, like colors:** an accessory's tags are its own plus
  its fabrics' tags, so a bow made of batik fabric is a batik bow without
  tagging it twice. Tags match ignoring capitals and plurals, with the same
  `singular` rule as suggestions. Fabric "keywords" are called tags in the
  interface (one idea, one name), but the column kept its name: renaming it
  would be a migration for no user benefit.
- **Filters live in the address** (`?tone=warm&sort=color`), so a filtered list
  can be bookmarked and the status buttons return to it. Table vs grid is a
  standing preference, so it's also remembered in a cookie.

## Colors: from free picking to a palette

This is the clearest example in the project of real use changing the design.

1. **Free picking drifted.** Each fabric and bow stored its own colors, and a
   color picker almost never lands on the same value twice. After a few months
   the data had **302 different shades for 84 names** (15 blacks, 14 reds).
   Suggestions only match names, so nothing broke, but the "pick a shade you've
   used before" list was unusable.
2. **First fix: a cleanup page.** A Shades page grouped look-alike shades so
   they could be merged into one (with a preview, and a backup first). Grouping
   by look alone turned out wrong for how the user thinks: a color with many
   shades was split across several groups, each mixed with different
   neighbors (31 names appeared in more than one group). Grouping by name
   first, with similar shades of *other* names as unticked extras, fixed that.
3. **The real fix: change the data model.** Cleanup treated the symptom;
   drift would come back. Now there's one **palette** of named colors, and
   fabrics and bows store links to it (in order). Renaming or reshading a color
   changes it everywhere, a color in use can't be deleted (a foreign key does
   that), and a new name joins the palette on purpose.
4. **The migration lost nothing.** Every name became a palette color with its
   most used shade. A name with clearly different shades kept all of them as
   "pink", "pink (2)", … to tidy up by hand, rather than the migration silently
   picking one. Colors without a shade joined their name's color. On a copy of
   the real data, all 372 colors were compared position by position before and
   after: 0 differences. The old JSON columns were dropped with `ALTER TABLE …
   DROP COLUMN`, which doesn't rebuild the table, so none of the cascade traps
   above applied.
5. **The old cleanup page became a smaller Palette page:** colors in rainbow
   order with what uses each, merge (with preview and backup), and colors that
   are hard to tell apart (ΔE under 5) pointed out, with "keep apart" for pairs
   that really differ (white vs "white sparkle" differ in finish, not shade).

**Backups before merges.** A merge rewrites many rows, so it first copies the
whole database with SQLite's `VACUUM INTO`, a consistent snapshot even while
the app runs. A test found that on an **in-memory** database `VACUUM INTO`
reports success but writes nothing, so the app now checks that the file
really exists before going ahead: a silent failure must never count as a
backup. Only the 10 newest automatic backups are kept.

**Color names.** The app's own 46 sewing names were replaced by the **xkcd
color survey** (names people actually typed for colors, public domain). The
Wikipedia lists were considered: they're under a share-alike license and
include proprietary names (Pantone, Crayola), and only some entries record
their source. The survey list needed cleaning: 41 vulgar or unhelpful names
and 39 duplicate spellings ("blue/green", "bluegreen", "blue green";
"terracota") were removed, each listed at the top of the data file, and a test
keeps every name to one spelling. The Palette page shows each color's closest
survey name as a second opinion, with "use it" (one click; it offers a merge
if another color already has that name) or "keep my name".

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
- **Beyond `cargo test`:** every schema change was tried on a *copy* of the
  real database first, comparing the data before and after (for the palette,
  every color, position by position).
- **Real browser checks**, in Firefox (the user's main browser) driven
  headlessly over its Marionette remote-control protocol with a ~50-line
  Python client, and in headless Chrome while Firefox couldn't be launched.
  Scripts that call the server are tested through a small same-origin proxy,
  so the cross-site protection is exercised for real instead of being switched
  off for the test. These checks caught bugs the Rust tests couldn't: a
  palette chip that changed the wrong row's shade, locked color swatches drawn
  faded (browsers dim disabled inputs, and the dot *was* the color), suggestion
  buttons with white text on a pale background, and a back-to-top button that
  visibly faded out on every page load.

## Known limitations, and what I'd do next

- **Single user, localhost only.** Sharing it would need authentication and
  per-user data; the repository layer is the natural seam for that.
- **HEIC and AVIF conversion needs macOS**: neither has a dependable pure-Rust
  decoder, so both are handed to macOS's `sips` tool and stored as JPEG. On
  other systems the upload page asks for a JPEG instead.
- **Color names are approximate** (the survey's name for a shade is one
  person's word for it), and a photo's background can show up as a suggested
  color. Both are one click to ignore or rename.
- **Name lists are built in:** the holidays and color names are data files
  read at build or start time; editing them means a restart or rebuild.
- **Next:** outfits in suggestions; photos in the suggestion box; a calendar
  nudge for upcoming events with no post yet; a suggested name for new bows
  built from the main fabric, kind and size.
