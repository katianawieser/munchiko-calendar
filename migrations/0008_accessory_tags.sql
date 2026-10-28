-- Migration 0008: tags on accessories ("batik", "japanese", "sparkly").
--
-- Fabrics already have keywords; accessories had nothing of their own, so a
-- bow could only be found through its name or its fabrics. Tags are a short
-- word list like keywords: a JSON array in one column. They're used to filter
-- the accessory list and by suggestions ("for Batik Day, consider ...").
--
-- Existing accessories start with no tags.

ALTER TABLE accessories ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(tags) AND json_type(tags) = 'array');
