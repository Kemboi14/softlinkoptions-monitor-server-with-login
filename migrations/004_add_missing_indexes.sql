-- Add missing database indexes for better performance

-- Composite index for alert deduplication queries (server_id, metric_type, is_resolved)
CREATE INDEX IF NOT EXISTS idx_alerts_server_metric_resolved ON alerts(server_id, metric_type, is_resolved);

-- Composite index for alert resolution queries (server_id, metric_type, is_resolved)
CREATE INDEX IF NOT EXISTS idx_alerts_server_type_resolved ON alerts(server_id, alert_type, is_resolved);

-- Composite index for server_stats queries (server_id, created_at) - improved for time-based queries
CREATE INDEX IF NOT EXISTS idx_server_stats_server_time_desc ON server_stats(server_id, created_at DESC);

-- Index for alert preferences user lookup
CREATE INDEX IF NOT EXISTS idx_alert_preferences_user_id ON alert_preferences(user_id);
