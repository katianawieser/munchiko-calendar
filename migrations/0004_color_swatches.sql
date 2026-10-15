-- Migration 0004: colors get an optional swatch.
--
-- Until now a color was just a word, saved as a JSON array of strings:
--     ["baby pink", "white"]
-- Words are vague ("baby pink" to whom?), so each color becomes a small
-- JSON object with its name and, optionally, the exact shade as #rrggbb:
--     [{"name": "baby pink", "hex": "#f4c2c2"}, {"name": "white"}]
--
-- The name stays required because suggestions match on it. Existing colors
-- keep their names and simply have no swatch yet.
--
-- How the conversion works, reading from the inside out:
-- * json_each(colors) turns the array into rows: key = position, value = word.
-- * json_object('name', value) wraps each word as {"name": word}.
-- * json_group_array(... ORDER BY key) collects them back into one array, in
--   the original order (it matters: a bow lists its main fabric's colors first).
-- * Over an empty array json_group_array gives '[]', so empty lists stay empty.

UPDATE fabrics SET colors = (
    SELECT json_group_array(json_object('name', value) ORDER BY key)
    FROM json_each(fabrics.colors)
);

UPDATE accessories SET colors = (
    SELECT json_group_array(json_object('name', value) ORDER BY key)
    FROM json_each(accessories.colors)
);
