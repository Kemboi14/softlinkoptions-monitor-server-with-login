#!/bin/bash
# Production Deployment Script with Safety Checks

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}🚀 Starting Production Deployment...${NC}"

# Configuration
APP_DIR="/var/www/monitor"
BACKUP_DIR="/var/www/monitor/backups"
SERVICE_NAME="rust-monitor"
PORT="5401"

# Safety checks
echo -e "${YELLOW}🔍 Running safety checks...${NC}"

# Check if we're on production server
if [ ! -d "$APP_DIR" ]; then
    echo -e "${RED}❌ Production directory not found: $APP_DIR${NC}"
    echo "Are you on the production server?"
    exit 1
fi

# Check if PM2 is running
if ! pm2 list | grep -q "$SERVICE_NAME"; then
    echo -e "${RED}❌ Service $SERVICE_NAME not found in PM2${NC}"
    exit 1
fi

# Check if current version is healthy
echo -e "${YELLOW}🏥 Checking current service health...${NC}"
if ! curl -f -s "http://localhost:$PORT/servers" > /dev/null; then
    echo -e "${RED}❌ Current service is not healthy!${NC}"
    echo "Please fix current version before deploying."
    exit 1
fi

# Create backup
echo -e "${YELLOW}📦 Creating backup...${NC}"
BACKUP_NAME="backup-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"

# Backup current application
sudo cp -r "$APP_DIR/current" "$BACKUP_DIR/$BACKUP_NAME"

# Backup database
sudo cp "$APP_DIR/data/monitor.db" "$BACKUP_DIR/$BACKUP_NAME/monitor.db"

echo -e "${GREEN}✅ Backup created: $BACKUP_NAME${NC}"

# Navigate to app directory
cd "$APP_DIR/current"

# Stash any local changes
git stash

# Pull latest code
echo -e "${YELLOW}📥 Pulling latest code...${NC}"
git fetch origin
git pull origin main

# Build application
echo -e "${YELLOW}🔨 Building application...${NC}"
cargo build --release

# Database migration (safe)
echo -e "${YELLOW}🗄️ Running database migrations...${NC}"
if [ -f "./migrate-production.sh" ]; then
    ./migrate-production.sh
else
    echo "No migration script found, skipping..."
fi

# Graceful reload with PM2
echo -e "${YELLOW}🔄 Reloading PM2 gracefully...${NC}"
pm2 reload "$SERVICE_NAME"

# Wait for application to start
echo -e "${YELLOW}⏳ Waiting for application to start...${NC}"
sleep 15

# Health check
echo -e "${YELLOW}🏥 Running health check...${NC}"
HEALTH_CHECK_URL="http://localhost:$PORT/servers"
MAX_RETRIES=5
RETRY_COUNT=0

while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if curl -f -s "$HEALTH_CHECK_URL" > /dev/null; then
        echo -e "${GREEN}✅ Health check passed!${NC}"
        break
    else
        echo -e "${YELLOW}⚠️ Health check failed, retrying... ($((RETRY_COUNT + 1))/$MAX_RETRIES)${NC}"
        sleep 10
        RETRY_COUNT=$((RETRY_COUNT + 1))
    fi
done

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo -e "${RED}❌ Health check failed after $MAX_RETRIES attempts!${NC}"
    echo -e "${RED}🔄 Rolling back...${NC}"
    
    # Rollback
    cd "$APP_DIR"
    sudo rm -rf "current"
    sudo cp -r "$BACKUP_DIR/$BACKUP_NAME" "current"
    cd "current"
    pm2 reload "$SERVICE_NAME"
    
    echo -e "${RED}❌ Deployment failed! Rolled back to previous version.${NC}"
    exit 1
fi

# Clean up old backups (keep last 5)
echo -e "${YELLOW}🧹 Cleaning up old backups...${NC}"
cd "$BACKUP_DIR"
ls -t | tail -n +6 | xargs -r rm -rf

echo -e "${GREEN}🎉 Production deployment successful!${NC}"
echo -e "${GREEN}📊 Service is running at: https://monitor.softlinkoptions.me.ke/${NC}"

# Show service status
pm2 status "$SERVICE_NAME"
