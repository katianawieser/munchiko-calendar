-- Migration 0013: the palette (M15a).
--
-- Until now each fabric and accessory kept its own list of colors (a JSON
-- array of {name, hex}), and every pick of the color picker made a slightly
-- different shade. Now there's one palette of named colors, and items point
-- at palette colors. Editing a palette color changes it everywhere, and a
-- palette color that's in use can't be deleted (ON DELETE NO ACTION).
--
-- Nothing is lost moving over:
--   * every name becomes a palette color, with its most used shade;
--   * a name used with several shades (three different "pink"s) gets one
--     palette color per shade: "pink", "pink (2)", "pink (3)", most used
--     first, to tidy up on the Palette page;
--   * a use of a name without a shade joins that name's (first) palette
--     color, which gives it a shade but takes nothing away;
--   * each item keeps its colors in the same order;
--   * names are lowercase, as the color forms have always saved them.

CREATE TABLE palette_colors (
    id   INTEGER PRIMARY KEY,
    -- Names are unique ignoring capitals: "Sage" and "sage" are one color.
    name TEXT    NOT NULL UNIQUE COLLATE NOCASE CHECK (name <> ''),
    -- Optional: "sparkly" can be a color without a shade.
    hex  TEXT    CHECK (hex IS NULL OR hex GLOB '#[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]')
) STRICT;

-- An item's colors, in order. Deleting the item deletes its links; a palette
-- color in use can't be deleted.
CREATE TABLE fabric_colors (
    fabric_id        INTEGER NOT NULL REFERENCES fabrics (id) ON DELETE CASCADE,
    position         INTEGER NOT NULL,
    palette_color_id INTEGER NOT NULL REFERENCES palette_colors (id) ON DELETE NO ACTION,
    PRIMARY KEY (fabric_id, position)
) STRICT;

CREATE TABLE accessory_colors (
    accessory_id     INTEGER NOT NULL REFERENCES accessories (id) ON DELETE CASCADE,
    position         INTEGER NOT NULL,
    palette_color_id INTEGER NOT NULL REFERENCES palette_colors (id) ON DELETE NO ACTION,
    PRIMARY KEY (accessory_id, position)
) STRICT;

-- "Which items use this color?" for the Palette page and deletes.
CREATE INDEX fabric_colors_by_color ON fabric_colors (palette_color_id);
CREATE INDEX accessory_colors_by_color ON accessory_colors (palette_color_id);

-- 1. Every color use, from both lists.
CREATE TEMP TABLE color_uses AS
    SELECT 'fabric' AS owner_kind, f.id AS owner_id, CAST(j.key AS INTEGER) AS position,
           trim(json_extract(j.value, '$.name')) AS name,
           lower(json_extract(j.value, '$.hex')) AS hex
    FROM fabrics f, json_each(f.colors) j
    UNION ALL
    SELECT 'accessory', a.id, CAST(j.key AS INTEGER),
           trim(json_extract(j.value, '$.name')),
           lower(json_extract(j.value, '$.hex'))
    FROM accessories a, json_each(a.colors) j;

-- 2. One palette color per name and shade. A name's shadeless uses are left
--    out when the name has a shade (they join it in step 3). Most used first.
CREATE TEMP TABLE palette_plan AS
    WITH pairs AS (
        SELECT lower(name) AS name_key, lower(name) AS name, hex, count(*) AS uses
        FROM color_uses
        WHERE name <> ''
        GROUP BY lower(name), hex
    ),
    kept AS (
        SELECT * FROM pairs p
        WHERE p.hex IS NOT NULL
           OR NOT EXISTS (SELECT 1 FROM pairs q WHERE q.name_key = p.name_key AND q.hex IS NOT NULL)
    )
    SELECT name_key, name, hex,
           ROW_NUMBER() OVER (PARTITION BY name_key ORDER BY uses DESC, hex) AS rank,
           ROW_NUMBER() OVER (ORDER BY name_key, uses DESC, hex) AS new_id
    FROM kept;

INSERT INTO palette_colors (id, name, hex)
    SELECT new_id, CASE WHEN rank = 1 THEN name ELSE name || ' (' || rank || ')' END, hex
    FROM palette_plan;

-- 3. Link every use to its palette color: same name and shade, or (no shade)
--    the name's first one. Positions keep the old order.
INSERT INTO fabric_colors (fabric_id, position, palette_color_id)
    SELECT u.owner_id, u.position, p.new_id
    FROM color_uses u
    JOIN palette_plan p ON p.name_key = lower(u.name)
                       AND (p.hex = u.hex OR (u.hex IS NULL AND p.rank = 1))
    WHERE u.owner_kind = 'fabric';

INSERT INTO accessory_colors (accessory_id, position, palette_color_id)
    SELECT u.owner_id, u.position, p.new_id
    FROM color_uses u
    JOIN palette_plan p ON p.name_key = lower(u.name)
                       AND (p.hex = u.hex OR (u.hex IS NULL AND p.rank = 1))
    WHERE u.owner_kind = 'accessory';

DROP TABLE color_uses;
DROP TABLE palette_plan;

-- 4. The old lists are now in the link tables.
ALTER TABLE fabrics DROP COLUMN colors;
ALTER TABLE accessories DROP COLUMN colors;
