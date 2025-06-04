-- Add is_hidden_from_pipes column to frames table
ALTER TABLE frames ADD COLUMN is_hidden_from_pipes BOOLEAN NOT NULL DEFAULT FALSE;

-- Optionally, update FTS triggers if needed, though likely not for this flag.
-- For example, if frames_fts included all columns and needed this:
-- DROP TRIGGER IF EXISTS frames_ai;
-- CREATE TRIGGER IF NOT EXISTS frames_ai AFTER INSERT ON frames BEGIN
--     INSERT INTO frames_fts(id, name, browser_url, app_name, window_name, focused, is_hidden_from_pipes)
--     VALUES (
--         NEW.id,
--         COALESCE(NEW.name, ''),
--         COALESCE(NEW.browser_url, ''),
--         COALESCE(NEW.app_name, ''),
--         COALESCE(NEW.window_name, ''),
--         COALESCE(NEW.focused, 0),
--         NEW.is_hidden_from_pipes
--     );
-- END;
-- Similar updates for frames_au and frames_ad triggers would be needed if including in FTS.
-- For now, we assume this flag is not part of FTS directly. 