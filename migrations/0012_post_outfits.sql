-- Migration 0012: which outfits a post was planned with.
--
-- Choosing an outfit copies its pieces into the post's looks, so the post
-- doesn't change when the outfit does. But when a piece is added to the
-- outfit later, the post page offers it ("Add the cape to Momo's look?").
-- For that, the post remembers each outfit chosen for each pet, and which
-- pieces the outfit had then (or were offered since). Pieces it didn't know
-- about are the new ones; pieces it knew but the pet isn't wearing were left
-- out on purpose and aren't offered again.

CREATE TABLE post_outfits (
    post_plan_id INTEGER NOT NULL REFERENCES post_plans (id) ON DELETE CASCADE,
    pet          TEXT    NOT NULL CHECK (pet IN ('mochi', 'momo')),
    -- Deleting the outfit forgets the choice; the looks stay.
    outfit_id    INTEGER NOT NULL REFERENCES outfits (id) ON DELETE CASCADE,
    PRIMARY KEY (post_plan_id, pet, outfit_id)
) STRICT;

CREATE TABLE post_outfit_known (
    post_plan_id INTEGER NOT NULL,
    pet          TEXT    NOT NULL,
    outfit_id    INTEGER NOT NULL,
    accessory_id INTEGER NOT NULL REFERENCES accessories (id) ON DELETE CASCADE,
    PRIMARY KEY (post_plan_id, pet, outfit_id, accessory_id),
    -- A foreign key over three columns: each row belongs to one choice above,
    -- and goes when it does.
    FOREIGN KEY (post_plan_id, pet, outfit_id)
        REFERENCES post_outfits (post_plan_id, pet, outfit_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX post_outfits_by_outfit ON post_outfits (outfit_id);
CREATE INDEX post_outfit_known_by_accessory ON post_outfit_known (accessory_id);

-- Like outfit_pieces (0011): a future migration that rebuilds post_plans
-- (like 0006) or accessories (like 0009) must save and restore these rows,
-- or dropping the old table will cascade here and empty them.
