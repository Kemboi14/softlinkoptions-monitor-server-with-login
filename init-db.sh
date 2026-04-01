#!/bin/bash

# Database initialization script
set -e

DB_PATH="${DATABASE_URL#sqlite:}"

echo "Initializing database at: $DB_PATH"

# Create database file if it doesn't exist
if [ ! -f "$DB_PATH" ]; then
    echo "Creating database file..."
    touch "$DB_PATH"
fi

# Run migrations
echo "Running database migrations..."

# Create tables
sqlite3 "$DB_PATH" << 'EOF'
-- Create servers table
CREATE TABLE IF NOT EXISTS servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    ip_address TEXT NOT NULL UNIQUE,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Create server_stats table
CREATE TABLE IF NOT EXISTS server_stats (
    id TEXT PRIMARY KEY,
    server_id TEXT NOT NULL,
    cpu_usage REAL NOT NULL,
    memory_usage REAL NOT NULL,
    memory_total REAL NOT NULL,
    disk_usage REAL NOT NULL,
    load_avg REAL NOT NULL,
    logged_users INTEGER NOT NULL,
    network_in REAL NOT NULL,
    network_out REAL NOT NULL,
    uptime INTEGER NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);

-- Create users table
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Create user_servers table
CREATE TABLE IF NOT EXISTS user_servers (
    user_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    PRIMARY KEY (user_id, server_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);

-- Create alerts table
CREATE TABLE IF NOT EXISTS alerts (
    id TEXT PRIMARY KEY,
    server_id TEXT NOT NULL,
    alert_type TEXT NOT NULL,
    metric_type TEXT NOT NULL,
    message TEXT NOT NULL,
    current_value REAL,
    threshold_value REAL,
    is_resolved BOOLEAN NOT NULL DEFAULT FALSE,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at DATETIME,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);

-- Create alert_preferences table
CREATE TABLE IF NOT EXISTS alert_preferences (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    cpu_threshold REAL NOT NULL DEFAULT 80.0,
    memory_threshold REAL NOT NULL DEFAULT 85.0,
    disk_threshold REAL NOT NULL DEFAULT 90.0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

-- Create rate_columns for network stats
ALTER TABLE server_stats ADD COLUMN network_in_rate REAL DEFAULT 0;
ALTER TABLE server_stats ADD COLUMN network_out_rate REAL DEFAULT 0;

-- Create push_metrics table
CREATE TABLE IF NOT EXISTS push_metrics (
    id TEXT PRIMARY KEY,
    server_id TEXT NOT NULL,
    api_key TEXT NOT NULL,
    last_seen DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);

-- Create indexes
CREATE INDEX IF NOT EXISTS idx_server_stats_server_id ON server_stats(server_id);
CREATE INDEX IF NOT EXISTS idx_server_stats_created_at ON server_stats(created_at);
CREATE INDEX IF NOT EXISTS idx_server_stats_server_created ON server_stats(server_id, created_at);
CREATE INDEX IF NOT EXISTS idx_alerts_server_id ON alerts(server_id);
CREATE INDEX IF NOT EXISTS idx_alerts_resolved ON alerts(is_resolved);
CREATE INDEX IF NOT EXISTS idx_user_servers_user_id ON user_servers(user_id);
CREATE INDEX IF NOT EXISTS idx_user_servers_server_id ON user_servers(server_id);
EOF

# Insert default admin user (password: admin123)
echo "Creating default admin user..."
ADMIN_PASSWORD_HASH='$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/LewdBPj6hsxq9w5GS'

sqlite3 "$DB_PATH" << EOF
INSERT OR IGNORE INTO users (id, username, password_hash, role) VALUES 
('admin-001', 'admin', '$ADMIN_PASSWORD_HASH', 'admin');
EOF

echo "Database initialization complete!"
