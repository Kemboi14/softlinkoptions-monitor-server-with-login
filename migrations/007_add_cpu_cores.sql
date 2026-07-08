-- Add cpu_cores column so load average alerts can be evaluated per-core.
ALTER TABLE server_stats ADD COLUMN cpu_cores INTEGER NOT NULL DEFAULT 0;
