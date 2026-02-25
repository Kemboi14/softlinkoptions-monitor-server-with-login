-- Create alerts table
CREATE TABLE IF NOT EXISTS alerts (
    id TEXT PRIMARY KEY,
    server_id TEXT NOT NULL,
    alert_type TEXT NOT NULL, -- 'critical', 'warning', 'down'
    metric_type TEXT NOT NULL, -- 'cpu', 'memory', 'disk', 'server_down'
    message TEXT NOT NULL,
    current_value REAL,
    threshold_value REAL,
    is_resolved INTEGER DEFAULT 0, -- Use INTEGER instead of BOOLEAN for SQLite consistency
    created_at DATETIME NOT NULL,
    resolved_at DATETIME,
    FOREIGN KEY (server_id) REFERENCES servers (id) ON DELETE CASCADE
);

-- Create alert preferences table
CREATE TABLE IF NOT EXISTS alert_preferences (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    cpu_threshold REAL DEFAULT 80.0,
    memory_threshold REAL DEFAULT 85.0,
    disk_threshold REAL DEFAULT 90.0,
    load_threshold REAL DEFAULT 10.0,
    enable_notifications INTEGER DEFAULT 1, -- Use INTEGER instead of BOOLEAN for SQLite consistency
    created_at DATETIME NOT NULL,
    updated_at DATETIME NOT NULL
);

-- Create indexes for better performance
CREATE INDEX IF NOT EXISTS idx_alerts_server_id ON alerts(server_id);
CREATE INDEX IF NOT EXISTS idx_alerts_type ON alerts(alert_type);
CREATE INDEX IF NOT EXISTS idx_alerts_resolved ON alerts(is_resolved);
CREATE INDEX IF NOT EXISTS idx_alerts_created_at ON alerts(created_at);
