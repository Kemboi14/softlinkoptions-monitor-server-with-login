# Client Installation Guide
## Softlink Options Server Monitoring System

**Monitor Server URL:** https://monitor.softlinkoptions.me.ke/

---

## Quick Overview

This guide will help you set up your server to send monitoring metrics to our central monitoring system. The entire process takes about 15-20 minutes.

**You have two options:**

### Option 1: Pull Mode (Recommended)
- Install Netdataon your server
- Open firewall port 19999
- Monitor server pulls metrics every 60 seconds
- **Best for:** Servers with public IP addresses

### Option 2: Push Mode
- Install a simple bash script
- Set up a cron job
- Your server pushes metrics every 60 seconds
- **Best for:** Servers behind firewall/NAT

---

## OPTION 1: Pull Mode Setup

### Step 1: Install Netdata

Run this single command on your server:

```bash
bash <(curl -Ss https://my-netdata.io/kickstart.sh)
```

Wait 2-5 minutes for installation to complete.

**Verify it's working:**
```bash
systemctl status netdata
```

You should see `Active: active (running)` in green.

---

### Step 2: Enable Remote Access

Edit Netdata configuration:

```bash
sudo nano /etc/netdata/netdata.conf
```

Find the `[web]` section and change:
```ini
[web]
    bind to = 0.0.0.0
```

**Or use this one-click command:**
```bash
sudo sed -i 's/bind to = localhost/bind to = 0.0.0.0/' /etc/netdata/netdata.conf
sudo systemctl restart netdata
```

---

### Step 3: Open Firewall Port

Choose based on your firewall:

**CSF Firewall (cPanel servers):**
```bash
sudo sed -i 's/^TCP_IN = "\(.*\)"/TCP_IN = "\1,19999"/' /etc/csf/csf.conf
sudo csf -r
```

**UFW (Ubuntu):**
```bash
sudo ufw allow 19999/tcp
sudo ufw reload
```

**Firewalld (CentOS/RHEL):**
```bash
sudo firewall-cmd --permanent --add-port=19999/tcp
sudo firewall-cmd --reload
```

**iptables:**
```bash
sudo iptables -A INPUT -p tcp --dport 19999 -j ACCEPT
sudo iptables-save > /etc/iptables/rules.v4
```

---

### Step 4: Test Connection

From another computer or the monitor server:

```bash
curl http://YOUR_SERVER_IP:19999/api/v1/info
```

You should see JSON output with Netdata version info.

---

### Step 5: Add to Monitor Dashboard

1. Go to https://monitor.softlinkoptions.me.ke/
2. Login with provided credentials
3. Click **"Add Server"**
4. Fill in:
   - **Server Name:** Your server's friendly name
   - **IP Address:** Your server's public IP
   - **Mode:** Pull Mode (default)
5. Click **"Add Server"**
6. Wait 60 seconds and refresh - done!

---

## OPTION 2: Push Mode Setup

### Step 1: Create the Push Script

```bash
sudo nano /usr/local/bin/push_metrics.sh
```

Copy the complete script from the `push_metrics_client.sh` file in this repository.

**Update the API key:**
```bash
API_KEY="YOUR_API_KEY_HERE"  # Get from admin
```

**Make it executable:**
```bash
sudo chmod +x /usr/local/bin/push_metrics.sh
```

---

### Step 2: Test the Script

```bash
sudo /usr/local/bin/push_metrics.sh
```

You should see:
```
[Date/Time] Metrics sent successfully
```

---

### Step 3: Schedule with Cron

```bash
sudo crontab -e
```

Add this line:
```cron
* * * * * /usr/local/bin/push_metrics.sh >> /var/log/push_metrics.log 2>&1
```

**Check it's running:**
```bash
tail -f /var/log/push_metrics.log
```

---

### Step 4: Add to Monitor Dashboard

1. Go to https://monitor.softlinkoptions.me.ke/
2. Login with provided credentials
3. Click **"Add Server"**
4. Fill in:
   - **Server Name:** Your server's friendly name
   - **IP Address:** Your server's public IP
   - **Mode:** **Push Mode** (important!)
5. Click **"Add Server"**
6. Wait 1-2 minutes and refresh - done!

---

## Verification Checklist

### For Pull Mode:
- [ ] Netdata is running: `systemctl status netdata`
- [ ] Port 19999 is accessible from external: `curl http://YOUR_IP:19999/api/v1/info`
- [ ] Server appears in dashboard
- [ ] Metrics are showing real values
- [ ] Status is green (🟢)

### For Push Mode:
- [ ] Script is executable: `ls -l /usr/local/bin/push_metrics.sh`
- [ ] Manual test succeeds
- [ ] Cron job is active: `sudo crontab -l`
- [ ] Logs show success: `tail /var/log/push_metrics.log`
- [ ] Server appears in dashboard with "Push Mode"
- [ ] Metrics are showing real values

---

## Troubleshooting

### Server not appearing?
- Wait 60-120 seconds after adding
- Refresh the browser (F5)
- Check IP address is correct: `curl ifconfig.me`

### Metrics showing 0% or N/A?
**Pull Mode:**
- Check firewall allows port 19999
- Restart Netdata: `sudo systemctl restart netdata`

**Push Mode:**
- Check logs: `tail /var/log/push_metrics.log`
- Verify API key is correct
- Test manual run

### Connection refused?
**Pull Mode:**
- Verify Netdata is listening: `ss -tlnp | grep 19999`
- Check bind address in config: `grep "bind to" /etc/netdata/netdata.conf`

**Push Mode:**
- Verify monitor server is reachable: `curl https://monitor.softlinkoptions.me.ke`
- Check curl is installed: `which curl`

---

## What Gets Monitored?

The system tracks these metrics every 60 seconds:

| Metric | Description |
|--------|-------------|
| **CPU Usage** | Percentage of CPU utilization |
| **Memory Usage** | RAM usage percentage and total GB |
| **Disk Usage** | Root partition usage percentage |
| **Load Average** | System load (1-minute average) |
| **Network** | Upload/download rates (bytes/sec) |
| **Disk I/O** | Read/write rates (bytes/sec) |
| **Uptime** | How long server has been running |
| **Logged Users** | Number of currently logged-in users |

---

## Alert Thresholds

Alerts are automatically generated when:

| Metric | Warning | Critical |
|--------|---------|----------|
| **CPU** | > 80% | > 95% |
| **Memory** | > 85% | > 95% |
| **Disk** | > 90% | > 98% |
| **Load Avg** | > 10.0 | > 20.0 |
| **Status** | - | Server DOWN |

---

## Need Help?

**Documentation:** See SOP.md for complete details

**Common Issues:**
- Port 19999 blocked → Check firewall rules
- Netdata not running → `sudo systemctl restart netdata`
- Script failing → Check API key and logs
- Metrics not updating → Wait 60 seconds and refresh

**Contact your system administrator for:**
- API keys for push mode
- Login credentials for dashboard
- Custom alert threshold configuration

---

## Summary

**You're done when:**
✅ Your server appears in the monitoring dashboard  
✅ All metrics (CPU, Memory, Disk) show real values  
✅ Status indicator is green (🟢)  
✅ History charts show data accumulating  

**What happens next:**
- Metrics collected every 60 seconds
- Historical data available for 7+ days
- Automatic alerts for threshold violations
- Real-time dashboard access 24/7

---

**Last Updated:** March 26, 2026  
**Version:** 1.0  
**Organization:** Softlink Options
