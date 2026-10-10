ALTER TABLE users ADD COLUMN show_hidden_files INTEGER NOT NULL DEFAULT 0
CHECK (show_hidden_files IN (0, 1));
