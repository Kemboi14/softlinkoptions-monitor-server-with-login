
ALTER TABLE server_stats ADD COLUMN network_in_rate REAL NOT NULL DEFAULT 0.0;
ALTER TABLE server_stats ADD COLUMN network_out_rate REAL NOT NULL DEFAULT 0.0;
ALTER TABLE server_stats ADD COLUMN disk_read_rate REAL NOT NULL DEFAULT 0.0;
ALTER TABLE server_stats ADD COLUMN disk_write_rate REAL NOT NULL DEFAULT 0.0;

-- Optional: index recent rates for faster queries
CREATE INDEX IF NOT EXISTS idx_server_stats_network_rates ON server_stats(server_id, created_at);
