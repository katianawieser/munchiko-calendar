-- Migration 0015: "keep my name" for palette colors.
--
-- The Palette page shows each color's closest everyday name (from the xkcd
-- survey) as a second opinion. Once you've decided to keep your own name,
-- the hint is just noise, so this remembers that choice per color. Existing
-- colors start without it (the hint shows until you keep the name).

ALTER TABLE palette_colors ADD COLUMN name_kept INTEGER NOT NULL DEFAULT 0
    CHECK (name_kept IN (0, 1));
