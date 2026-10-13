-- Migration 0003: let an accessory take its colors from its fabrics.
--
-- Typing "orange, black" for a bow made of orange velvet and black satin is
-- repetitive. With colors_from_fabrics = 1, the app works out the bow's
-- colors from its fabrics every time it needs them (so editing a fabric's
-- colors updates every bow made from it), and the `colors` column holds only
-- extra colors on top, such as a white embroidered trim.
--
-- Existing accessories get 0, so they keep exactly the colors already typed.

ALTER TABLE accessories ADD COLUMN colors_from_fabrics INTEGER NOT NULL DEFAULT 0
    CHECK (colors_from_fabrics IN (0, 1));
