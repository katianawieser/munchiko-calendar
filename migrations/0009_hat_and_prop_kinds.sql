-- Migration 0009: two more kinds of accessory, hats and props.
--
-- Crowns, witch hats and party hats are "hat"; a wand, a sign or a basket the
-- pets pose with is a "prop". The list of kinds is a CHECK rule on the table,
-- and SQLite can't change a CHECK in place, so the table is rebuilt the same
-- careful way as in migration 0006:
--   * `accessory_fabrics` and `photos` point at accessories with ON DELETE
--     CASCADE, so dropping the old table would delete them; `looks` points at
--     it with NO ACTION, so dropping it would fail. Save them, take them out,
--     rebuild, put them back.
--   * New trap here: the table points at itself (`pair_id`, a matching set).
--     If the new table said `REFERENCES accessories`, dropping the OLD
--     accessories table would "SET NULL" every pair in the new one. So the new
--     table refers to itself as `accessories_new`; the RENAME at the end
--     rewrites that to `accessories`.
-- It all runs inside sqlx's transaction: if any step fails, nothing changes.

-- 1. Save the rows that point at accessories.
CREATE TEMP TABLE saved_accessory_fabrics AS
    SELECT rowid AS old_order, accessory_id, fabric_id, role FROM accessory_fabrics;
CREATE TEMP TABLE saved_looks AS
    SELECT rowid AS old_order, post_plan_id, pet, accessory_id FROM looks;
CREATE TEMP TABLE saved_accessory_photos AS
    SELECT id, accessory_id, content_type FROM photos WHERE accessory_id IS NOT NULL;

-- 2. Take them out, so dropping the table has nothing to cascade to.
DELETE FROM accessory_fabrics;
DELETE FROM looks;
DELETE FROM photos WHERE accessory_id IS NOT NULL;

-- 3. Rebuild accessories with the longer list of kinds, keeping every id.
CREATE TABLE accessories_new (
    id          INTEGER PRIMARY KEY,
    kind        TEXT    NOT NULL
                CHECK (kind IN ('bow', 'scarf', 'costume', 'hat', 'prop')),
    name        TEXT    NOT NULL CHECK (name <> ''),
    description TEXT    NOT NULL DEFAULT '',
    colors      TEXT    NOT NULL DEFAULT '[]'
                CHECK (json_valid(colors) AND json_type(colors) = 'array'),
    colors_from_fabrics INTEGER NOT NULL DEFAULT 0 CHECK (colors_from_fabrics IN (0, 1)),
    tags        TEXT    NOT NULL DEFAULT '[]'
                CHECK (json_valid(tags) AND json_type(tags) = 'array'),
    source      TEXT    NOT NULL CHECK (source IN ('handmade', 'bought')),
    brand       TEXT,
    status      TEXT    NOT NULL CHECK (status IN ('idea', 'in_progress', 'ready')),
    -- The matching accessory in a Mochi & Momo set (two-way; see 0001).
    pair_id     INTEGER REFERENCES accessories_new (id) ON DELETE SET NULL,
    -- NULL means "use the default for this kind".
    make_days   INTEGER CHECK (make_days > 0),
    CHECK (source = 'bought' OR brand IS NULL),
    CHECK (pair_id <> id)
) STRICT;

-- One statement, so SQLite checks the pair_id links once all rows are in.
INSERT INTO accessories_new
    (id, kind, name, description, colors, colors_from_fabrics, tags, source, brand,
     status, pair_id, make_days)
    SELECT id, kind, name, description, colors, colors_from_fabrics, tags, source, brand,
           status, pair_id, make_days
    FROM accessories;

DROP TABLE accessories;
ALTER TABLE accessories_new RENAME TO accessories;

CREATE UNIQUE INDEX accessories_one_partner ON accessories (pair_id)
    WHERE pair_id IS NOT NULL;

-- 4. Put everything back, in the original order. Same accessory ids, so
--    they point at the same accessories; photos keep their ids and files.
INSERT INTO accessory_fabrics (accessory_id, fabric_id, role)
    SELECT accessory_id, fabric_id, role FROM saved_accessory_fabrics ORDER BY old_order;
INSERT INTO looks (post_plan_id, pet, accessory_id)
    SELECT post_plan_id, pet, accessory_id FROM saved_looks ORDER BY old_order;
INSERT INTO photos (id, accessory_id, content_type)
    SELECT id, accessory_id, content_type FROM saved_accessory_photos;

DROP TABLE saved_accessory_fabrics;
DROP TABLE saved_looks;
DROP TABLE saved_accessory_photos;
