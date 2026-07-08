#!/bin/bash
# ============================================================
# Safe production update script for rust-monitor-server
# Run this ON the production server after git pull.
# ============================================================
set -e

DB_PATH="${DATABASE_URL:-sqlite:./monitor.db}"
DB_FILE="${DB_PATH#sqlite:}"   # strip "sqlite:" prefix

SERVICE_NAME="${SERVICE_NAME:-rust-monitor}"  # override with your systemd unit name

echo "=== Softlink Options Server Monitor — Safe Deploy ==="
echo "DB: $DB_FILE"
echo "Service: $SERVICE_NAME"
echo ""

# ── 1. Back up the database ──────────────────────────────────
BACKUP="$DB_FILE.backup-$(date +%Y%m%d-%H%M%S)"
echo "[1/5] Backing up database → $BACKUP"
cp "$DB_FILE" "$BACKUP"

# ── 2. Apply only the NEW column (migration 007) ─────────────
echo "[2/5] Applying migration 007 (cpu_cores column) if not present..."
python3 - "$DB_FILE" << 'PYEOF'
import sqlite3, sys
db = sys.argv[1]
conn = sqlite3.connect(db)
cur = conn.cursor()
cur.execute("PRAGMA table_info(server_stats)")
cols = [row[1] for row in cur.fetchall()]
if 'cpu_cores' not in cols:
    cur.execute("ALTER TABLE server_stats ADD COLUMN cpu_cores INTEGER NOT NULL DEFAULT 0")
    conn.commit()
    print("  cpu_cores column added.")
else:
    print("  cpu_cores column already exists — skipping.")
conn.close()
PYEOF

# ── 3. Seed the sqlx migration tracker ───────────────────────
echo "[3/5] Seeding _sqlx_migrations table so sqlx skips already-applied migrations..."
python3 - "$DB_FILE" << 'PYEOF'
import sqlite3, sys
db = sys.argv[1]
conn = sqlite3.connect(db)
conn.execute("""
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version        INTEGER  PRIMARY KEY,
    description    TEXT     NOT NULL,
    installed_on   TEXT     NOT NULL,
    success        INTEGER  NOT NULL,
    checksum       BLOB     NOT NULL,
    execution_time INTEGER  NOT NULL
)
""")
conn.execute("""
INSERT OR IGNORE INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES
  (1, 'create_tables',     datetime('now'), 1, X'3983e255a95afdf7d2ba53cc063df65bfba3d156b5b9c4158d0727c1e84f87e90df691ef80409c87c4fff395cdd5f7cd', 0),
  (2, 'add_rate_columns',  datetime('now'), 1, X'a2ae487f41eb2b5ddce73be9b01abd1ac13f9212e5beb2203fd6272b464699c6f3f15ed6fd0ddcc6afda1a0a18a34cc0', 0),
  (3, 'create_alerts',     datetime('now'), 1, X'eab7c9de6562b4ec33a27f5eff8f6511ec72f17cc592e5d36e8a1bd9f9559459b2a9a7dd8fcbada1f459b5e3ded1caa7', 0),
  (4, 'add_missing_indexes',datetime('now'),1, X'1ac0a0ee2a20d07b2cc83f36368c8a0ec8f19920f75f285a7d9a092fda906b2b36666a01162011681a9bddf2ae0ffa63', 0),
  (5, 'add_push_metrics',  datetime('now'), 1, X'03aad242d20344b85608f69c296436a1a2eee20f67f392dccb1c2b7e884b79e02375c697876cf1a87e38d80a4b94861c', 0),
  (6, 'add_users',         datetime('now'), 1, X'b6a63d535b37c93edf80bd839147e18e6b36b6aac6830fa44f7b41f131c7d0cac4f246b250ee1b9d0b1df0e0f55aaa49', 0),
  (7, 'add_cpu_cores',     datetime('now'), 1, X'ee300dbebfbbf252bae2247cbff2cb0a3b4f525878e0720139b7ff1f87b1d1365608eb8606f49d96097275b2ac188df4', 0)
""")
conn.commit()
conn.close()
print("  _sqlx_migrations seeded.")
PYEOF

# ── 4. Build the release binary ──────────────────────────────
echo "[4/5] Building release binary..."
export DATABASE_URL="$DB_PATH"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release

# ── 5. Restart the service ───────────────────────────────────
echo "[5/5] Restarting service: $SERVICE_NAME"
echo "      (if using systemd, run: sudo systemctl restart $SERVICE_NAME)"
echo "      If running manually, stop the old process and start:"
echo "        nohup ./target/release/rust-monitor-server &"

echo ""
echo "=== Deploy complete. Database backup at: $BACKUP ==="
