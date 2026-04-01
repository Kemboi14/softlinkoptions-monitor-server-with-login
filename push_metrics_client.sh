#!/bin/bash

###############################################################################
# Server Metrics Push Script
# For Softlink Options Monitor Server
# Version: 1.0
###############################################################################

# Configuration - UPDATE THESE VALUES
MONITOR_URL="https://monitor.softlinkoptions.me.ke/api/metrics/submit"
API_KEY="YOUR_API_KEY_HERE"  # Get this from your monitor server's .env file

# Get server IP (auto-detect public IP)
SERVER_IP=$(curl -s ifconfig.me || curl -s icanhazip.com || hostname -I | awk '{print $1}')

###############################################################################
# Function: Get CPU Usage
###############################################################################
get_cpu_usage() {
    # Get CPU usage as percentage (average over 1 second)
    top -bn2 -d 0.5 | grep "Cpu(s)" | tail -n1 | awk '{print $2}' | sed 's/%us,//'
}

###############################################################################
# Function: Get Memory Usage
###############################################################################
get_memory_stats() {
    # Returns: used_percentage total_gb
    free -m | awk 'NR==2{printf "%.1f %.1f", $3*100/$2, $2/1024}'
}

###############################################################################
# Function: Get Disk Usage
###############################################################################
get_disk_usage() {
    df -h / | awk 'NR==2{print $5}' | sed 's/%//'
}

###############################################################################
# Function: Get Load Average
###############################################################################
get_load_avg() {
    uptime | awk -F'load average:' '{print $2}' | awk -F, '{print $1}' | xargs
}

###############################################################################
# Function: Get Logged Users
###############################################################################
get_logged_users() {
    who | wc -l
}

###############################################################################
# Function: Get Network Stats (bytes/sec)
###############################################################################
get_network_stats() {
    # Sample network usage twice with 1 second interval
    local iface=$(ip route | grep default | awk '{print $5}' | head -n1)
    
    if [ -z "$iface" ]; then
        echo "0 0"
        return
    fi
    
    local rx_bytes_1=$(cat /sys/class/net/$iface/statistics/rx_bytes 2>/dev/null || echo 0)
    local tx_bytes_1=$(cat /sys/class/net/$iface/statistics/tx_bytes 2>/dev/null || echo 0)
    
    sleep 1
    
    local rx_bytes_2=$(cat /sys/class/net/$iface/statistics/rx_bytes 2>/dev/null || echo 0)
    local tx_bytes_2=$(cat /sys/class/net/$iface/statistics/tx_bytes 2>/dev/null || echo 0)
    
    local rx_rate=$((rx_bytes_2 - rx_bytes_1))
    local tx_rate=$((tx_bytes_2 - tx_bytes_1))
    
    echo "$rx_rate $tx_rate"
}

###############################################################################
# Function: Get Disk I/O Stats (bytes/sec)
###############################################################################
get_disk_io_stats() {
    # Get primary disk device
    local disk=$(df / | tail -1 | awk '{print $1}' | sed 's|/dev/||' | sed 's/[0-9]*$//')
    
    if [ -f "/sys/block/$disk/stat" ]; then
        local stats_1=$(cat /sys/block/$disk/stat)
        local read_sectors_1=$(echo $stats_1 | awk '{print $3}')
        local write_sectors_1=$(echo $stats_1 | awk '{print $7}')
        
        sleep 1
        
        local stats_2=$(cat /sys/block/$disk/stat)
        local read_sectors_2=$(echo $stats_2 | awk '{print $3}')
        local write_sectors_2=$(echo $stats_2 | awk '{print $7}')
        
        # Convert sectors to bytes (512 bytes per sector)
        local read_rate=$(( (read_sectors_2 - read_sectors_1) * 512 ))
        local write_rate=$(( (write_sectors_2 - write_sectors_1) * 512 ))
        
        echo "$read_rate $write_rate"
    else
        echo "0 0"
    fi
}

###############################################################################
# Function: Get Uptime (in seconds)
###############################################################################
get_uptime() {
    cat /proc/uptime | awk '{print int($1)}'
}

###############################################################################
# Main: Collect and Send Metrics
###############################################################################

# Collect all metrics
CPU_USAGE=$(get_cpu_usage)
MEMORY_STATS=$(get_memory_stats)
MEMORY_USAGE=$(echo $MEMORY_STATS | awk '{print $1}')
MEMORY_TOTAL=$(echo $MEMORY_STATS | awk '{print $2}')
DISK_USAGE=$(get_disk_usage)
LOAD_AVG=$(get_load_avg)
LOGGED_USERS=$(get_logged_users)
NETWORK_STATS=$(get_network_stats)
NETWORK_IN=$(echo $NETWORK_STATS | awk '{print $1}')
NETWORK_OUT=$(echo $NETWORK_STATS | awk '{print $2}')
DISK_IO_STATS=$(get_disk_io_stats)
DISK_READ=$(echo $DISK_IO_STATS | awk '{print $1}')
DISK_WRITE=$(echo $DISK_IO_STATS | awk '{print $2}')
UPTIME=$(get_uptime)

# Build JSON payload
JSON_PAYLOAD=$(cat <<EOF
{
  "ip_address": "$SERVER_IP",
  "cpu_usage": $CPU_USAGE,
  "memory_usage": $MEMORY_USAGE,
  "memory_total": $MEMORY_TOTAL,
  "disk_usage": $DISK_USAGE,
  "load_avg": $LOAD_AVG,
  "logged_users": $LOGGED_USERS,
  "network_in_rate": $NETWORK_IN,
  "network_out_rate": $NETWORK_OUT,
  "disk_read_rate": $DISK_READ,
  "disk_write_rate": $DISK_WRITE,
  "uptime": $UPTIME
}
EOF
)

# Send metrics to monitor server
RESPONSE=$(curl -s -X POST "$MONITOR_URL" \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: $API_KEY" \
  -d "$JSON_PAYLOAD")

# Log result (optional)
if echo "$RESPONSE" | grep -q "success"; then
    echo "[$(date)] Metrics sent successfully"
else
    echo "[$(date)] Failed to send metrics: $RESPONSE" >&2
    exit 1
fi
