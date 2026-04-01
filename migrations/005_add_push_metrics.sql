-- Add push_metrics flag to servers table
-- When TRUE, the scheduler will skip polling this server via Netdata:19999
-- and rely solely on the server pushing its own metrics via POST /api/metrics/submit
ALTER TABLE servers ADD COLUMN push_metrics INTEGER NOT NULL DEFAULT 0;
