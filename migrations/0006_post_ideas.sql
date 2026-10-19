-- Migration 0006: post ideas without a date, and posts without an event.
--
-- Some posts don't belong to a day yet ("Pokémon fossil exhibit": bows
-- planned, backdrop props bought, no date), and some have no event at all.
-- So post_plans gets a `title`, and `event_id` and `post_date` become optional:
--   * no post_date = an idea, waiting to be scheduled;
--   * no event_id  = a post that stands on its own (it then needs a title).
--
-- Why this file is more involved than the others:
-- SQLite can't remove NOT NULL from a column in place, so the table is rebuilt:
-- create the new shape, copy the rows, drop the old table, rename the new one.
-- The trap: `looks` and `photos` point at post_plans with ON DELETE CASCADE,
-- so dropping the old table would also delete every look and every post photo.
-- SQLite's own recipe switches foreign keys off first, but that setting can't
-- change inside a transaction, and sqlx always runs SQLite migrations inside
-- one (its `-- no-transaction` option isn't supported for SQLite in sqlx 0.8).
--
-- So instead: save the child rows, take them out, rebuild, put them back.
-- It all happens in sqlx's transaction, so if any step fails, nothing changes.

-- 1. Save the looks (in their original order) and the photos of posts.
CREATE TEMP TABLE saved_looks AS
    SELECT rowid AS old_order, post_plan_id, pet, accessory_id FROM looks;
CREATE TEMP TABLE saved_post_photos AS
    SELECT id, post_plan_id, content_type FROM photos WHERE post_plan_id IS NOT NULL;

-- 2. Take them out ourselves, so dropping the table has nothing to cascade to.
DELETE FROM looks;
DELETE FROM photos WHERE post_plan_id IS NOT NULL;

-- 3. Rebuild post_plans in its new shape, keeping every id.
CREATE TABLE post_plans_new (
    id        INTEGER PRIMARY KEY,
    -- Optional for posts with an event (the event's name is used); required
    -- for posts without one, by the CHECK at the bottom.
    title     TEXT    NOT NULL DEFAULT '',
    event_id  INTEGER REFERENCES events (id) ON DELETE NO ACTION,
    -- NULL = not scheduled yet. A date must still be a real date.
    post_date TEXT    CHECK (post_date IS NULL OR post_date = date(post_date)),
    backdrop  TEXT    NOT NULL DEFAULT '',
    notes     TEXT    NOT NULL DEFAULT '',
    CHECK (event_id IS NOT NULL OR title <> '')
) STRICT;

INSERT INTO post_plans_new (id, event_id, post_date, backdrop, notes)
    SELECT id, event_id, post_date, backdrop, notes FROM post_plans;

DROP TABLE post_plans;
ALTER TABLE post_plans_new RENAME TO post_plans;

CREATE INDEX post_plans_by_event ON post_plans (event_id);
CREATE INDEX post_plans_by_date ON post_plans (post_date);

-- 4. Put the looks and photos back. Same post ids, so they point at the
--    same posts; the photos keep their ids, so their files still match.
INSERT INTO looks (post_plan_id, pet, accessory_id)
    SELECT post_plan_id, pet, accessory_id FROM saved_looks ORDER BY old_order;
INSERT INTO photos (id, post_plan_id, content_type)
    SELECT id, post_plan_id, content_type FROM saved_post_photos;

DROP TABLE saved_looks;
DROP TABLE saved_post_photos;
