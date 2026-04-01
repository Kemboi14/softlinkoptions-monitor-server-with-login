#!/bin/bash
# Database Backup Script

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}📦 Creating database backup...${NC}"

# Configuration
APP_DIR="/var/www/monitor"
BACKUP_DIR="/var/www/monitor/backups"
DB_PATH="$APP_DIR/data/monitor.db"

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Create backup filename with timestamp
BACKUP_FILE="monitor-db-$(date +%Y%m%d-%H%M%S).db"
BACKUP_PATH="$BACKUP_DIR/$BACKUP_FILE"

# Backup database
if [ -f "$DB_PATH" ]; then
    cp "$DB_PATH" "$BACKUP_PATH"
    echo -e "${GREEN}✅ Database backed up to: $BACKUP_PATH${NC}"
    
    # Show backup size
    BACKUP_SIZE=$(du -h "$BACKUP_PATH" | cut -f1)
    echo -e "${GREEN}📊 Backup size: $BACKUP_SIZE${NC}"
    
    # Clean up old database backups (keep last 10)
    cd "$BACKUP_DIR"
    ls -t monitor-db-*.db | tail -n +11 | xargs -r rm -f
    echo -e "${GREEN}🧹 Old backups cleaned up${NC}"
    
else
    echo -e "${YELLOW}⚠️ Database file not found: $DB_PATH${NC}"
    exit 1
fi

echo -e "${GREEN}🎉 Database backup completed!${NC}"
