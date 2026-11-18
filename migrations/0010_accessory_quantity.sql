-- Migration 0010: how many of each accessory there are ("×2").
--
-- One number per accessory, not one row per copy: two identical bows are
-- the same item twice, and nothing about them differs that we'd track.
-- It lets both pets wear the same bow in one post, and the app warns when a
-- post needs more copies than there are (see `PostPlan::shortages`).
--
-- For an accessory that isn't made yet it's how many will be made.
-- Existing accessories start at 1, which is what they meant until now.
-- Adding a column with a constant default doesn't rebuild the table, so no
-- child rows need saving (unlike migrations 0006 and 0009).

ALTER TABLE accessories ADD COLUMN quantity INTEGER NOT NULL DEFAULT 1
    CHECK (quantity >= 1);
