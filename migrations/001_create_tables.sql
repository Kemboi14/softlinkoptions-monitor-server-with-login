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

-- Create indexes for performance
CREATE INDEX IF NOT EXISTS idx_server_stats_server_id ON server_stats(server_id);
CREATE INDEX IF NOT EXISTS idx_server_stats_created_at ON server_stats(created_at);
CREATE INDEX IF NOT EXISTS idx_server_stats_server_created ON server_stats(server_id, created_at);
