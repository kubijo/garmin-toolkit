ALTER TABLE users ADD COLUMN inline_file_windows INTEGER NOT NULL DEFAULT 0
CHECK (inline_file_windows IN (0, 1));
