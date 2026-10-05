-- Migration 0001: the initial schema.
--
-- A migration is a numbered SQL script that moves the database from one
-- version to the next. sqlx records which ones have already run (in its own
-- `_sqlx_migrations` table), so each script runs exactly once per database.
-- To change the schema later we add 0002_..., we never edit this file.
--
-- Conventions used throughout:
-- * Tables are STRICT: SQLite refuses to store the wrong type in a column.
--   (By default SQLite would quietly accept the text 'abc' in an INTEGER column.)
-- * `id INTEGER PRIMARY KEY` makes SQLite assign the next free number when we
--   insert a row without an id.
-- * Dates are TEXT in ISO format, 'YYYY-MM-DD'. That format sorts correctly as
--   plain text, and the check `x = date(x)` rejects anything that isn't a real
--   calendar date (date('2026-02-30') is NULL, so the check fails).
-- * Enums are lowercase TEXT limited by CHECK (column IN (...)).
-- * Short word lists (colors, keywords) are JSON arrays in one TEXT column,
--   e.g. '["orange","black"]'. The CHECK makes sure it really is a JSON array.
-- * REFERENCES declares a foreign key: the value must be an id that exists in
--   the other table. ON DELETE says what happens when that other row is deleted:
--     CASCADE   - delete this row too
--     NO ACTION - refuse the delete while this row still points at it
--     SET NULL  - keep this row but clear the link
--   (We use NO ACTION rather than RESTRICT. Both block the delete, but SQLite
--   reports RESTRICT with a trigger error code that sqlx doesn't recognise as
--   a foreign-key failure, so the app couldn't tell the user "still in use".)

-- Fabric in the stash, or on the shopping list when in_stock = 0.
CREATE TABLE fabrics (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL CHECK (name <> ''),
    description TEXT    NOT NULL DEFAULT '',
    colors      TEXT    NOT NULL DEFAULT '[]'
                CHECK (json_valid(colors) AND json_type(colors) = 'array'),
    keywords    TEXT    NOT NULL DEFAULT '[]'
                CHECK (json_valid(keywords) AND json_type(keywords) = 'array'),
    in_stock    INTEGER NOT NULL DEFAULT 1 CHECK (in_stock IN (0, 1))
) STRICT;

-- "These two fabrics look good together." Pairings go both ways, so the app
-- stores both directions: pairing A with B inserts (A, B) and (B, A). That
-- keeps "what pairs with fabric X?" a simple WHERE fabric_id = X.
CREATE TABLE fabric_pairings (
    fabric_id        INTEGER NOT NULL REFERENCES fabrics (id) ON DELETE CASCADE,
    paired_fabric_id INTEGER NOT NULL REFERENCES fabrics (id) ON DELETE CASCADE,
    PRIMARY KEY (fabric_id, paired_fabric_id),
    CHECK (fabric_id <> paired_fabric_id)
) STRICT;

-- A bow, scarf or costume.
CREATE TABLE accessories (
    id          INTEGER PRIMARY KEY,
    kind        TEXT    NOT NULL CHECK (kind IN ('bow', 'scarf', 'costume')),
    name        TEXT    NOT NULL CHECK (name <> ''),
    description TEXT    NOT NULL DEFAULT '',
    colors      TEXT    NOT NULL DEFAULT '[]'
                CHECK (json_valid(colors) AND json_type(colors) = 'array'),
    source      TEXT    NOT NULL CHECK (source IN ('handmade', 'bought')),
    brand       TEXT,
    status      TEXT    NOT NULL CHECK (status IN ('idea', 'in_progress', 'ready')),
    -- The matching accessory in a Mochi & Momo set. Like fabric pairings this
    -- is two-way: both rows point at each other. Deleting one half clears the
    -- other half's link instead of deleting it.
    pair_id     INTEGER REFERENCES accessories (id) ON DELETE SET NULL,
    -- NULL means "use the default for this kind".
    make_days   INTEGER CHECK (make_days > 0),
    -- Only bought items have a brand.
    CHECK (source = 'bought' OR brand IS NULL),
    CHECK (pair_id <> id)
) STRICT;

-- Each accessory has at most one partner: no two rows may point at the same
-- pair_id. ("WHERE pair_id IS NOT NULL" makes this a partial index, so any
-- number of unpaired accessories are still allowed.)
CREATE UNIQUE INDEX accessories_one_partner ON accessories (pair_id)
    WHERE pair_id IS NOT NULL;

-- Which fabrics an accessory is made from, and where each one goes.
-- A fabric can't be deleted while an accessory still uses it.
CREATE TABLE accessory_fabrics (
    accessory_id INTEGER NOT NULL REFERENCES accessories (id) ON DELETE CASCADE,
    fabric_id    INTEGER NOT NULL REFERENCES fabrics (id) ON DELETE NO ACTION,
    role         TEXT    NOT NULL CHECK (role IN ('main', 'middle', 'accent')),
    PRIMARY KEY (accessory_id, fabric_id, role)
) STRICT;

-- SQLite doesn't index foreign keys automatically. Without this index, every
-- fabric delete would scan the whole table to check nothing still uses it.
CREATE INDEX accessory_fabrics_by_fabric ON accessory_fabrics (fabric_id);

-- Holidays, notable days, seasons and outings. Single-day events have no end_date.
CREATE TABLE events (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL CHECK (name <> ''),
    description TEXT NOT NULL DEFAULT '',
    kind        TEXT NOT NULL
                CHECK (kind IN ('holiday', 'notable_day', 'seasonal', 'outing')),
    start_date  TEXT NOT NULL CHECK (start_date = date(start_date)),
    end_date    TEXT CHECK (end_date = date(end_date) AND end_date >= start_date),
    keywords    TEXT NOT NULL DEFAULT '[]'
                CHECK (json_valid(keywords) AND json_type(keywords) = 'array')
) STRICT;

-- The calendar view asks "what's happening between these two dates?"
CREATE INDEX events_by_start ON events (start_date);

-- A planned post. An event can't be deleted while a post is planned for it.
CREATE TABLE post_plans (
    id        INTEGER PRIMARY KEY,
    event_id  INTEGER NOT NULL REFERENCES events (id) ON DELETE NO ACTION,
    post_date TEXT    NOT NULL CHECK (post_date = date(post_date)),
    backdrop  TEXT    NOT NULL DEFAULT '',
    notes     TEXT    NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX post_plans_by_event ON post_plans (event_id);
CREATE INDEX post_plans_by_date ON post_plans (post_date);

-- Who wears what in a post. A pet can wear several things (a bow and a scarf),
-- so the key is the whole (post, pet, accessory) combination.
-- Deleting the post deletes its looks; an accessory that's in a planned look
-- can't be deleted.
CREATE TABLE looks (
    post_plan_id INTEGER NOT NULL REFERENCES post_plans (id) ON DELETE CASCADE,
    pet          TEXT    NOT NULL CHECK (pet IN ('mochi', 'momo')),
    accessory_id INTEGER NOT NULL REFERENCES accessories (id) ON DELETE NO ACTION,
    PRIMARY KEY (post_plan_id, pet, accessory_id)
) STRICT;

CREATE INDEX looks_by_accessory ON looks (accessory_id);
