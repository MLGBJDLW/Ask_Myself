-- Add metadata column to documents table for storing extracted metadata
-- (frontmatter fields, filesystem dates, author, tags, etc.).
-- JSON-serialized HashMap<String, String>.
ALTER TABLE documents ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}';
