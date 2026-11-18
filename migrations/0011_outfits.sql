-- Migration 0011: outfits, pieces that are worn together.
--
-- Glinda is a crown, a dress and a wand. Each piece stays its own accessory
-- (with its own status, fabrics, prep time and quantity); an outfit is a
-- named group of them, with notes and photos of the whole look.
--
-- A piece can be in several outfits (the same wand for Glinda and a fairy),
-- so pieces are a join table rather than an outfit_id column on accessories.
--
-- Choosing an outfit in a post copies its pieces into the post's looks, so
-- posts don't point at outfits at all, and deleting an outfit never touches
-- a planned post.

CREATE TABLE outfits (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL CHECK (name <> ''),
    description TEXT    NOT NULL DEFAULT ''
) STRICT;

-- ON DELETE CASCADE both ways: deleting an outfit removes its piece list
-- (not the accessories), and deleting an accessory takes it out of its
-- outfits. Neither is worth blocking a delete for.
--
-- Watch out in any future migration that rebuilds `accessories` (like 0009):
-- dropping the old table would cascade here and empty every outfit, so save
-- these rows first and put them back afterwards.
CREATE TABLE outfit_pieces (
    outfit_id    INTEGER NOT NULL REFERENCES outfits (id) ON DELETE CASCADE,
    accessory_id INTEGER NOT NULL REFERENCES accessories (id) ON DELETE CASCADE,
    PRIMARY KEY (outfit_id, accessory_id)
) STRICT;

-- "Which outfits is this bow in?" for the accessory page.
CREATE INDEX outfit_pieces_by_accessory ON outfit_pieces (accessory_id);

-- Photos get a fourth kind of owner. Its "exactly one owner" CHECK can't be
-- changed in place, so the table is rebuilt. Nothing references photos, so
-- dropping it cascades nowhere. Two things to keep:
--   * every row with its id (the files on disk are named after the id);
--   * the AUTOINCREMENT counter, so an id of a photo deleted earlier is never
--     handed out again. It lives in sqlite_sequence, and dropping the table
--     drops its counter, so it's saved and restored.
CREATE TEMP TABLE saved_photo_counter AS
    SELECT seq FROM sqlite_sequence WHERE name = 'photos';

CREATE TABLE photos_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    fabric_id     INTEGER REFERENCES fabrics (id) ON DELETE CASCADE,
    accessory_id  INTEGER REFERENCES accessories (id) ON DELETE CASCADE,
    post_plan_id  INTEGER REFERENCES post_plans (id) ON DELETE CASCADE,
    outfit_id     INTEGER REFERENCES outfits (id) ON DELETE CASCADE,
    content_type  TEXT    NOT NULL
                  CHECK (content_type IN ('image/jpeg', 'image/png', 'image/webp', 'image/gif')),
    CHECK ((fabric_id IS NOT NULL) + (accessory_id IS NOT NULL) + (post_plan_id IS NOT NULL)
           + (outfit_id IS NOT NULL) = 1)
) STRICT;

INSERT INTO photos_new (id, fabric_id, accessory_id, post_plan_id, content_type)
    SELECT id, fabric_id, accessory_id, post_plan_id, content_type FROM photos ORDER BY id;

DROP TABLE photos;
ALTER TABLE photos_new RENAME TO photos;

DELETE FROM sqlite_sequence WHERE name IN ('photos', 'photos_new');
INSERT INTO sqlite_sequence (name, seq) SELECT 'photos', seq FROM saved_photo_counter;
DROP TABLE saved_photo_counter;

CREATE INDEX photos_by_fabric ON photos (fabric_id);
CREATE INDEX photos_by_accessory ON photos (accessory_id);
CREATE INDEX photos_by_post_plan ON photos (post_plan_id);
CREATE INDEX photos_by_outfit ON photos (outfit_id);
