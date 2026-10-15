-- Migration 0005: photos of fabrics, accessories and post plans.
--
-- The image files themselves live on disk (the photos/ folder, named
-- <id>.<extension>); this table records which photo belongs to what.
-- Keeping multi-megabyte images out of SQLite keeps the database small and
-- quick to back up, and lets the files be served straight from disk.
--
-- Who owns a photo: one column per kind of owner, and the CHECK at the
-- bottom requires exactly one of them to be filled in. (In SQL, a comparison
-- like `fabric_id IS NOT NULL` is 1 or 0, so adding them up counts the
-- filled-in columns.) The alternative, a single "owner_id" plus an
-- "owner_kind" column, couldn't use REFERENCES, so the database couldn't
-- check that the owner exists.
--
-- ON DELETE CASCADE: deleting a fabric deletes its photo rows too. The app
-- then removes the files.
--
-- AUTOINCREMENT: without it SQLite may reuse the id of the most recently
-- deleted row, and a browser that cached the old /photos/<id> image would
-- show it for the new photo. With it, ids are never reused.

CREATE TABLE photos (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    fabric_id     INTEGER REFERENCES fabrics (id) ON DELETE CASCADE,
    accessory_id  INTEGER REFERENCES accessories (id) ON DELETE CASCADE,
    post_plan_id  INTEGER REFERENCES post_plans (id) ON DELETE CASCADE,
    content_type  TEXT    NOT NULL
                  CHECK (content_type IN ('image/jpeg', 'image/png', 'image/webp', 'image/gif')),
    CHECK ((fabric_id IS NOT NULL) + (accessory_id IS NOT NULL) + (post_plan_id IS NOT NULL) = 1)
) STRICT;

-- "All photos of fabric 3" is the common question; these make it quick.
CREATE INDEX photos_by_fabric ON photos (fabric_id);
CREATE INDEX photos_by_accessory ON photos (accessory_id);
CREATE INDEX photos_by_post_plan ON photos (post_plan_id);
