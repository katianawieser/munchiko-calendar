-- Migration 0014: palette colors that look alike on purpose.
--
-- The Palette page points out colors that are hard to tell apart (a merge
-- candidate). Some are meant to differ in a way a shade can't show: "white"
-- and "white sparkle" differ in finish, not color. "Keep apart" records the
-- pair so it stops being pointed out. Merging or deleting either color takes
-- the pair with it (CASCADE). Each pair is stored once, smaller id first.

CREATE TABLE palette_kept_apart (
    color_a INTEGER NOT NULL REFERENCES palette_colors (id) ON DELETE CASCADE,
    color_b INTEGER NOT NULL REFERENCES palette_colors (id) ON DELETE CASCADE,
    PRIMARY KEY (color_a, color_b),
    CHECK (color_a < color_b)
) STRICT;
