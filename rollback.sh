#!/bin/bash
# Emergency Rollback Script

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${RED}🚨 Emergency Rollback Started${NC}"

# Configuration
APP_DIR="/var/www/monitor"
BACKUP_DIR="/var/www/monitor/backups"
SERVICE_NAME="rust-monitor"

# Find the latest backup
LATEST_BACKUP=$(ls -t "$BACKUP_DIR" | head -n 1)

if [ -z "$LATEST_BACKUP" ]; then
    echo -e "${RED}❌ No backups found!${NC}"
    exit 1
fi

echo -e "${YELLOW}📦 Rolling back to: $LATEST_BACKUP${NC}"

# Stop current service
echo -e "${YELLOW}⏹️ Stopping current service...${NC}"
pm2 stop "$SERVICE_NAME"

# Restore application
echo -e "${YELLOW}🔄 Restoring application...${NC}"
cd "$APP_DIR"
sudo rm -rf "current"
sudo cp -r "$BACKUP_DIR/$LATEST_BACKUP" "current"

# Restore database if backup exists
if [ -f "$BACKUP_DIR/$LATEST_BACKUP/monitor.db" ]; then
    echo -e "${YELLOW}🗄️ Restoring database...${NC}"
    sudo cp "$BACKUP_DIR/$LATEST_BACKUP/monitor.db" "$APP_DIR/data/monitor.db"
fi

# Start service
echo -e "${YELLOW}▶️ Starting service...${NC}"
cd "$APP_DIR/current"
pm2 start "$SERVICE_NAME"

# Wait for service to start
sleep 10

# Health check
echo -e "${YELLOW}🏥 Running health check...${NC}"
if curl -f -s "http://localhost:5401/servers" > /dev/null; then
    echo -e "${GREEN}✅ Rollback successful!${NC}"
    echo -e "${GREEN}📊 Service is running at: https://monitor.softlinkoptions.me.ke/${NC}"
else
    echo -e "${RED}❌ Rollback failed! Service is not healthy.${NC}"
    echo "Please check logs: pm2 logs $SERVICE_NAME"
    exit 1
fi

echo -e "${GREEN}🎉 Emergency rollback completed!${NC}"
