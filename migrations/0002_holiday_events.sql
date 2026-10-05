-- Migration 0002: link events to the holidays file.
--
-- Holidays from data/holidays.toml are stored as ordinary rows in `events`
-- (post plans point at events by id, so holidays need real rows too). These
-- two columns record which holiday and which year a row was made from, so the
-- app can update that row when the file changes instead of adding a duplicate.
--
-- Events you enter yourself (outings) leave both columns NULL, and the sync
-- never touches them.

ALTER TABLE events ADD COLUMN holiday_key TEXT;     -- the holiday's name in the file
ALTER TABLE events ADD COLUMN holiday_year INTEGER;

-- One row per holiday per year. We key on the year rather than the date so
-- that when a date changes (Shark Week gets announced, a typo is fixed) the
-- existing row is moved, and any post plan attached to it stays attached.
-- Partial index: rows with no holiday_key (outings) aren't limited at all.
CREATE UNIQUE INDEX events_one_per_holiday_year ON events (holiday_key, holiday_year)
    WHERE holiday_key IS NOT NULL;
