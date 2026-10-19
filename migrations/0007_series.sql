-- Migration 0007: series, several posts from one setup and one photo shoot.
--
-- Earth Day might be three posts: Momo on her own, Mochi on his own, then
-- both together. They share one backdrop and one photo session, and go up on
-- different days. A series holds what they share (the backdrop); each post
-- keeps its own date, looks and title.
--
-- Adding a column that may be NULL is something SQLite can do in place
-- (unlike 0006's change), so no rebuild is needed here.
--
-- ON DELETE SET NULL: deleting a series keeps its posts; they just stop being
-- part of a series. (The app copies the series' backdrop onto them first, so
-- the idea isn't lost.)

CREATE TABLE series (
    id       INTEGER PRIMARY KEY,
    title    TEXT    NOT NULL CHECK (title <> ''),
    backdrop TEXT    NOT NULL DEFAULT '',
    notes    TEXT    NOT NULL DEFAULT ''
) STRICT;

ALTER TABLE post_plans ADD COLUMN series_id INTEGER REFERENCES series (id) ON DELETE SET NULL;

CREATE INDEX post_plans_by_series ON post_plans (series_id);
