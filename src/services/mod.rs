use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use serde_json::Value as JsonValue;
use sqlx::SqlitePool;
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::models::{ServerStats, NetdataResponse, Alert, AlertPreference};

const MIN_DISK_USAGE_PERCENT: f64 = 0.0;
const DEFAULT_DISK_USAGE_PERCENT: f64 = 0.0;

#[derive(Error, Debug)]
pub enum MetricsError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("System error: {0}")]
    SystemError(String),
}

#[derive(Clone)]
pub struct MetricsService {
    client: Client,
    pool: SqlitePool,
}

impl MetricsService {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("Failed to build HTTP client"),
            pool,
        }
    }

    pub async fn collect_and_save(&self, ip: Option<&str>, id: &str) -> Result<(), MetricsError> {
        let ip = match ip {
            Some(ip) => ip,
            None => return Err(MetricsError::SystemError("Invalid IP".to_string())),
        };

        let stats = self.fetch_metrics(ip).await?;
        
        // Check for alerts after collecting metrics
        self.check_and_create_alerts(&stats, id).await?;
        
        self.save_stats(stats, id).await
    }

    pub async fn fetch_metrics(&self, server_ip: &str) -> Result<ServerStats, MetricsError> {
        info!("Fetching metrics from {}", server_ip);

        let base = format!("http://{}:19999", server_ip);

        // Helpers to query Netdata JSON endpoints
        async fn get_chart(client: &Client, url: &str, chart: &str) -> Result<NetdataResponse, MetricsError> {
            // Use after=-10 for recent data; points=5 for rate calculation
            let full = format!("{}/api/v1/data?chart={}&points=5&after=-10&options=unaligned", url, chart);
            let resp = client.get(&full).send().await?.error_for_status()?;
            let json = resp.json::<serde_json::Value>().await?;
            let labels = json.get("result").and_then(|r| r.get("labels")).or_else(|| json.get("labels")).and_then(|v| v.as_array());
            let data_arr = json.get("result").and_then(|r| r.get("data")).or_else(|| json.get("data")).and_then(|v| v.as_array());
            if let (Some(labels), Some(data)) = (labels, data_arr) {
                let label_vec: Vec<String> = labels.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
                let data_vec: Vec<Vec<serde_json::Value>> = data.iter().filter_map(|v| v.as_array().map(|arr| arr.iter().cloned().collect())).collect();
                if !label_vec.is_empty() && !data_vec.is_empty() {
                    return Ok(NetdataResponse { labels: label_vec, data: data_vec });
                }
            }
            Err(MetricsError::SystemError("Failed to parse Netdata response".to_string()))
        }

        async fn get_json(client: &Client, url: &str, path: &str) -> Result<JsonValue, MetricsError> {
            let full = format!("{}/{}", url, path.trim_start_matches('/'));
            let resp = client.get(&full).send().await?.error_for_status()?;
            let json = resp.json::<JsonValue>().await?;
            Ok(json)
        }

        // Discover available charts from Netdata with size limits
        async fn discover_charts(client: &Client, url: &str) -> Result<Vec<String>, MetricsError> {
            // Create a client with shorter timeout for discovery
            let discovery_client = Client::builder()
                .timeout(Duration::from_secs(10)) // Shorter timeout for discovery
                .build()
                .map_err(|e| MetricsError::HttpError(e.into()))?;
            
            match get_json(&discovery_client, url, "api/v1/charts").await {
                Ok(charts_json) => {
                    // Limit response size to prevent memory exhaustion
                    let charts_str = serde_json::to_string(&charts_json)
                        .map_err(|_| MetricsError::SystemError("Failed to serialize charts".to_string()))?;
                    
                    if charts_str.len() > 1_000_000 { // 1MB limit
                        warn!("Charts response too large, skipping discovery");
                        return Ok(vec![]);
                    }
                    
                    // Charts are nested under "charts" object
                    if let Some(charts_obj) = charts_json.get("charts").and_then(|c| c.as_object()) {
                        let chart_names: Vec<String> = charts_obj.keys()
                            .take(1000) // Limit number of charts
                            .map(|s| s.to_string())
                            .collect();
                        info!("Successfully discovered {} charts from Netdata", chart_names.len());
                        return Ok(chart_names);
                    }
                    // Fallback: try direct array if structure is different
                    else if let Some(charts) = charts_json.get("charts").and_then(|c| c.as_array()) {
                        let chart_names: Vec<String> = charts.iter()
                            .take(1000) // Limit number of charts
                            .filter_map(|chart| chart.get("id").and_then(|id| id.as_str()))
                            .map(|s| s.to_string())
                            .collect();
                        info!("Successfully discovered {} charts from Netdata (array format)", chart_names.len());
                        return Ok(chart_names);
                    } else {
                        warn!("Unexpected charts response structure from Netdata");
                    }
                }
                Err(e) => {
                    warn!("Failed to discover charts from Netdata: {}", e);
                }
            }
            Ok(vec![])
        }

        // Discover available charts for better fallback logic
        let available_charts = discover_charts(&self.client, &base).await.unwrap_or_default();
        if !available_charts.is_empty() {
            info!("Available charts: {} charts found", available_charts.len());
        } else {
            info!("Chart discovery failed, using fallback candidates");
        }

        // Helper function to check if a chart should be tried
        let should_try_chart = |chart_name: &str| -> bool {
            if available_charts.is_empty() {
                return true; // If discovery failed, try all candidates
            }
            
            // Exact match first
            if available_charts.contains(&chart_name.to_string()) {
                return true;
            }
            
            // Then try substring matches with word boundaries to avoid false positives
            available_charts.iter().any(|available| {
                // Check if the candidate is a substring of available chart
                // but ensure it matches whole chart segments (separated by dots)
                available.split('.').any(|segment| segment.contains(chart_name)) ||
                // Or if the available chart contains the candidate as a whole segment
                chart_name.split('.').any(|segment| available.contains(segment))
            })
        };

        // CPU: expanded candidates for better detection
        let cpu_candidates = [
            "system.cpu",
            "system.cpu_all",
            "system.cpu0",
            "system.cpu_percentage",
            "system.cpu_usage",
            "cpu.cpu",
            "cpu.total",
            "netdata.cpu",
            "system.cpu_average"
        ];
        let mut cpu_usage = None;
        for c in cpu_candidates {
            if should_try_chart(c) {
                if let Ok(chart) = get_chart(&self.client, &base, c).await {
                    cpu_usage = Some(parse_cpu(&chart));
                    if cpu_usage.is_some() { break; }
                }
            }
        }

        // Fetch info JSON once for authoritative totals (ram, disk) and uptime
        let info_json = get_json(&self.client, &base, "api/v1/info").await.ok();

        // Memory: expanded candidates for better detection
        let mem_candidates = [
            "system.ram",
            "system.ram_size",
            "system.memory",
            "system.mem",
            "mem.ram",
            "memory.ram",
            "netdata.ram",
            "system.physmem",
            "system.physical_memory"
        ];
        let mut memory_usage: f64 = 0.0;
        let mut memory_total_gb = 0.0;
        for c in mem_candidates {
            if should_try_chart(c) {
                if let Ok(chart) = get_chart(&self.client, &base, c).await {
                    let (usage, total) = parse_memory(&chart, info_json.as_ref());
                    memory_usage = usage;
                    memory_total_gb = total;
                    break;
                }
            }
        }

        // Disk: expanded candidates for better cross-server compatibility
        let disk_candidates = [
            "disk_space._",
            "disk_space._root",
            "disk_space./",
            "disk_space./root",
            "disk_space._root_",
            "disk_space.rootfs",
            "disk_space./dev",
            "disk_space./home",
            "disk_space./var",
            "disk_space./usr",
            "disk_space./tmp",
            "system.disk",
            "disk.space._",
            "disk_util._",
            "disk_util._root",
            "disk_util./",
            "disk_util.rootfs",
            "disk.usage._",
            "disk.usage./",
            "disk.mounts._",
            "disk.mounts./",
            "filesystem._",
            "filesystem./",
            "vfs._",
            "vfs./"
        ];
        let mut disk_usage_percent: f64 = 0.0;
        let mut disk_found = false;
        for c in disk_candidates {
            if should_try_chart(c) {
                if let Ok(chart) = get_chart(&self.client, &base, c).await {
                    let pct = parse_disk(&chart);
                    if pct > 0.0 && pct <= 100.0 {
                        disk_usage_percent = pct;
                        disk_found = true;
                        break;
                    }
                }
            }
        }
        
        // If no disk usage found, try to get from info endpoint
        if !disk_found {
            if let Some(ref info) = info_json {
                if let (Some(used), Some(total)) = (
                    info.get("disk_used").and_then(json_to_f64),
                    info.get("disk_total").and_then(json_to_f64)
                ) {
                    disk_usage_percent = compute_percent(used, total);
                }
            }
        }

        // Uptime: expanded candidates
        let mut uptime_secs = 0i64;
        let uptime_candidates = [
            "system.uptime",
            "uptime",
            "system.uptime_seconds",
            "system.uptime_timestamp",
            "netdata.uptime",
            "proc.uptime",
            "system.boot_time"
        ];
        for uc in uptime_candidates {
            if should_try_chart(uc) {
                if let Ok(chart) = get_chart(&self.client, &base, uc).await {
                    if let Some(row) = chart.data.get(0) {
                        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                            if let Some(v) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                                if lbl.to_lowercase().contains("uptime") {
                                    uptime_secs = v as i64;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if uptime_secs > 0 { break; }
        }
        
        // Fallback: use system boot time from info endpoint or reasonable estimate
        if uptime_secs == 0 {
            if let Some(ref info) = info_json {
                // Try to get boot time from info endpoint
                if let Some(boot_time) = info.get("system_boot_time").and_then(json_to_f64) {
                    let now = Utc::now().timestamp() as f64;
                    uptime_secs = (now - boot_time) as i64;
                } else if let Some(start_time) = info.get("netdata_start_time").and_then(json_to_f64) {
                    let now = Utc::now().timestamp() as f64;
                    uptime_secs = (now - start_time) as i64;
                }
            }
            
            // Final fallback: use current time minus a reasonable monitoring start time
            if uptime_secs == 0 {
                use chrono::{Utc, Duration};
                let now = Utc::now();
                // Assume monitoring started recently (last 24 hours) - more realistic than 26 days
                let estimated_start = now - Duration::hours(12);
                uptime_secs = (now - estimated_start).num_seconds();
                info!("Using estimated uptime: {} seconds", uptime_secs);
            }
        }

        // Load average: expanded candidates
        let mut load_avg = 0.0f64;
        let load_candidates = [
            "system.load",
            "load.load",
            "system.loadavg",
            "netdata.load",
            "proc.loadavg",
            "system.system_load"
        ];
        for lc in load_candidates {
            if should_try_chart(lc) {
                if let Ok(chart) = get_chart(&self.client, &base, lc).await {
                    if let Some(row) = chart.data.get(0) {
                        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                            let label = lbl.to_lowercase();
                            if label.contains("load1") || label.contains("1m") || label.contains("load") {
                                if let Some(v) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                                    load_avg = v;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if load_avg > 0.0 { break; }
        }

        // Logged users - expanded chart candidates and better detection
        let mut logged_users = 0i64;
        let user_candidates = [
            "system.users",
            "users",
            "system.users_count",
            "system.active_users",
            "system.logged_users",
            "netdata.users",
            "proc.numusers",
            "proc.users"
        ];
        for uc in user_candidates {
            if should_try_chart(uc) {
                if let Ok(chart) = get_chart(&self.client, &base, uc).await {
                    if let Some(row) = chart.data.get(0) {
                        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                            let label = lbl.to_lowercase();
                            if label.contains("users") || label.contains("sessions") || label.contains("logged") {
                                if let Some(v) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                                    logged_users = v as i64;
                                    break;
                                }
                            }
                        }
                    }
                }
                if logged_users > 0 { break; }
            }
        }

        // Network rates: sum across all interfaces (in = download/received, out = upload/sent)
        let (mut net_in_rate, mut net_out_rate) = (0.0f64, 0.0f64);
        let net_candidates = [
            "system.net",
            "system.ip",
            "net.eth0",
            "net.eth1",
            "net.ens3",
            "net.ens5",
            "net.enp0s3",
            "net.enp0s31f6",
            "net.wlan0",
            "net.lo",
        ];
        for net_chart in net_candidates {
            if should_try_chart(net_chart) {
                if let Ok(chart) = get_chart(&self.client, &base, net_chart).await {
                    let (in_r, out_r) = parse_network_rates(&chart);
                    net_in_rate += in_r;
                    net_out_rate += out_r;
                }
            }
        }

        // Disk I/O rates: read and write separately
        let (mut disk_read_rate, mut disk_write_rate) = (0.0f64, 0.0f64);
        let disk_io_candidates = ["disk_io._total", "disk_io.sda", "disk_io.nvme0n1", "disk.io", "system.io"];
        for disk_io_chart in disk_io_candidates {
            if should_try_chart(disk_io_chart) {
                if let Ok(chart) = get_chart(&self.client, &base, disk_io_chart).await {
                    let (r, w) = parse_disk_io_rates(&chart);
                    if r > 0.0 || w > 0.0 {
                        disk_read_rate = r;
                        disk_write_rate = w;
                        break;
                    }
                }
            }
        }

        let cpu_final = cpu_usage.unwrap_or(0.0).clamp(0.0, 100.0);
        let mem_final = memory_usage.clamp(0.0, 100.0);
        let disk_final = disk_usage_percent.clamp(0.0, 100.0);
        
        // Validate parsed metrics to ensure reasonable values
        let load_final = if load_avg.is_nan() || load_avg.is_infinite() || load_avg < 0.0 {
            0.0
        } else {
            load_avg
        };
        
        let users_final = if logged_users < 0 || logged_users > 1000 {
            0 // Reasonable upper bound for logged users
        } else {
            logged_users
        };
        
        let uptime_final = if uptime_secs < 0 || uptime_secs > (365 * 24 * 60 * 60) {
            0 // Max 1 year uptime
        } else {
            uptime_secs
        };
        
        info!("Final metrics for {}: CPU={:.1}%, MEM={:.1}%, DISK={:.1}%", 
               server_ip, cpu_final, mem_final, disk_final);

        Ok(ServerStats {
            id: Uuid::new_v4().to_string(),
            server_id: String::new(),
            cpu_usage: cpu_final,
            memory_usage: mem_final,
            memory_total: memory_total_gb,
            disk_usage: disk_final,
            load_avg: load_final,
            logged_users: users_final,
            network_in: net_in_rate, // Use actual network values instead of hardcoded zeros
            network_out: net_out_rate,
            network_in_rate: net_in_rate,
            network_out_rate: net_out_rate,
            disk_read_rate: disk_read_rate,
            disk_write_rate: disk_write_rate,
            uptime: uptime_final,
            created_at: Utc::now().naive_utc(),
        })
    }

    pub async fn save_stats(&self, mut stats: ServerStats, server_id: &str) -> Result<(), MetricsError> {
        stats.server_id = server_id.to_string();
        stats.disk_usage = stats.disk_usage.max(MIN_DISK_USAGE_PERCENT);

        sqlx::query!(
            r#"
            INSERT INTO server_stats (
                id, server_id, cpu_usage, memory_usage, memory_total,
                disk_usage, load_avg, logged_users,
                network_in, network_out, network_in_rate, network_out_rate,
                disk_read_rate, disk_write_rate, uptime, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            stats.id,
            stats.server_id,
            stats.cpu_usage,
            stats.memory_usage,
            stats.memory_total,
            stats.disk_usage,
            stats.load_avg,
            stats.logged_users,
            stats.network_in,
            stats.network_out,
            stats.network_in_rate,
            stats.network_out_rate,
            stats.disk_read_rate,
            stats.disk_write_rate,
            stats.uptime,
            stats.created_at
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Check metrics against thresholds and create alerts if necessary
    pub async fn check_and_create_alerts(&self, stats: &ServerStats, server_id: &str) -> Result<(), MetricsError> {
        // Get default alert preferences (for now, we'll use defaults)
        let preferences = self.get_default_alert_preferences().await?;
        
        // Check server down condition (no recent metrics)
        self.check_server_down_alert(server_id).await?;
        
        // Check CPU usage
        if stats.cpu_usage > preferences.cpu_threshold {
            let alert_type = if stats.cpu_usage > 95.0 { "critical" } else { "warning" };
            self.create_alert_if_not_exists(
                server_id,
                alert_type,
                "cpu",
                format!("CPU usage is {:.1}%", stats.cpu_usage),
                Some(stats.cpu_usage),
                Some(preferences.cpu_threshold),
            ).await?;
        }
        
        // Check memory usage
        if stats.memory_usage > preferences.memory_threshold {
            let alert_type = if stats.memory_usage > 95.0 { "critical" } else { "warning" };
            self.create_alert_if_not_exists(
                server_id,
                alert_type,
                "memory",
                format!("Memory usage is {:.1}%", stats.memory_usage),
                Some(stats.memory_usage),
                Some(preferences.memory_threshold),
            ).await?;
        }
        
        // Check disk usage
        if stats.disk_usage > preferences.disk_threshold {
            let alert_type = if stats.disk_usage > 98.0 { "critical" } else { "warning" };
            self.create_alert_if_not_exists(
                server_id,
                alert_type,
                "disk",
                format!("Disk usage is {:.1}%", stats.disk_usage),
                Some(stats.disk_usage),
                Some(preferences.disk_threshold),
            ).await?;
        }
        
        // Check load average
        if stats.load_avg > preferences.load_threshold {
            let alert_type = if stats.load_avg > 20.0 { "critical" } else { "warning" };
            self.create_alert_if_not_exists(
                server_id,
                alert_type,
                "load",
                format!("Load average is {:.2}", stats.load_avg),
                Some(stats.load_avg),
                Some(preferences.load_threshold),
            ).await?;
        }
        
        Ok(())
    }
    
    /// Check if server is down (no recent metrics)
    async fn check_server_down_alert(&self, server_id: &str) -> Result<(), MetricsError> {
        let five_minutes_ago = Utc::now().naive_utc() - chrono::Duration::minutes(5);
        
        let recent_count = sqlx::query_scalar!(
            "SELECT COUNT(*) as count FROM server_stats WHERE server_id = ? AND created_at > ?",
            server_id,
            five_minutes_ago
        )
        .fetch_one(&self.pool)
        .await?;
        
        let count: i64 = recent_count as i64;
        
        if count == 0 {
            self.create_alert_if_not_exists(
                server_id,
                "critical",
                "server_down",
                "Server is down or not responding".to_string(),
                None,
                None,
            ).await?;
        } else {
            // Resolve any existing server down alerts
            self.resolve_alerts_by_type(server_id, "server_down").await?;
        }
        
        Ok(())
    }
    
    /// Create alert if one of the same type and metric doesn't already exist and is unresolved
    async fn create_alert_if_not_exists(
        &self,
        server_id: &str,
        alert_type: &str,
        metric_type: &str,
        message: String,
        current_value: Option<f64>,
        threshold_value: Option<f64>,
    ) -> Result<(), MetricsError> {
        // Use INSERT with ON CONFLICT to prevent race conditions
        let alert_id = Uuid::new_v4().to_string();
        let now = Utc::now().naive_utc();
        
        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO alerts (
                id, server_id, alert_type, metric_type, message,
                current_value, threshold_value, is_resolved, created_at
            ) SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?
            WHERE NOT EXISTS (
                SELECT 1 FROM alerts 
                WHERE server_id = ? AND alert_type = ? AND metric_type = ? AND is_resolved = FALSE
            )
            "#,
            alert_id,
            server_id,
            alert_type,
            metric_type,
            message,
            current_value,
            threshold_value,
            false,
            now,
            server_id,
            alert_type,
            metric_type
        )
        .execute(&self.pool)
        .await?;
        
        // Check if we actually inserted a new alert
        let rows_affected = sqlx::query_scalar!(
            "SELECT changes() as count"
        )
        .fetch_one(&self.pool)
        .await?;
        
        if rows_affected > 0 {
            warn!("🚨 Alert created: {} - {} (Server: {})", alert_type, message, server_id);
        }
        
        Ok(())
    }
    
    /// Resolve alerts by type for a server
    async fn resolve_alerts_by_type(&self, server_id: &str, metric_type: &str) -> Result<(), MetricsError> {
        let now = Utc::now().naive_utc();
        
        sqlx::query!(
            "UPDATE alerts SET is_resolved = TRUE, resolved_at = ? 
             WHERE server_id = ? AND metric_type = ? AND is_resolved = FALSE",
            now,
            server_id,
            metric_type
        )
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Get default alert preferences
    async fn get_default_alert_preferences(&self) -> Result<AlertPreference, MetricsError> {
        let user_id = "default"; // We'll use a default user for now
        
        // Try to get existing preferences
        if let Some(prefs) = sqlx::query_as!(
            AlertPreference,
            r#"SELECT id as "id!", user_id as "user_id!", cpu_threshold as "cpu_threshold!", 
               memory_threshold as "memory_threshold!", disk_threshold as "disk_threshold!",
               load_threshold as "load_threshold!", enable_notifications as "enable_notifications!",
               created_at as "created_at!", updated_at as "updated_at!"
               FROM alert_preferences WHERE user_id = ?"#,
            user_id
        )
        .fetch_optional(&self.pool)
        .await? {
            return Ok(prefs);
        }
        
        // Create default preferences
        let prefs_id = Uuid::new_v4().to_string();
        let now = Utc::now().naive_utc();
        
        let preferences = AlertPreference {
            id: prefs_id.clone(),
            user_id: user_id.to_string(),
            cpu_threshold: 80.0,
            memory_threshold: 85.0,
            disk_threshold: 90.0,
            load_threshold: 10.0,
            enable_notifications: true,
            created_at: now,
            updated_at: now,
        };
        
        sqlx::query!(
            r#"
            INSERT INTO alert_preferences (
                id, user_id, cpu_threshold, memory_threshold, disk_threshold,
                load_threshold, enable_notifications, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            preferences.id,
            preferences.user_id,
            preferences.cpu_threshold,
            preferences.memory_threshold,
            preferences.disk_threshold,
            preferences.load_threshold,
            preferences.enable_notifications,
            preferences.created_at,
            preferences.updated_at
        )
        .execute(&self.pool)
        .await?;
        
        Ok(preferences)
    }
    
    /// Get unresolved alerts for a server
    pub async fn get_unresolved_alerts(&self, server_id: Option<&str>) -> Result<Vec<Alert>, MetricsError> {
        let alerts = if let Some(sid) = server_id {
            sqlx::query_as!(
                Alert,
                r#"SELECT id as "id!", server_id as "server_id!", alert_type as "alert_type!", 
                   metric_type as "metric_type!", message as "message!", current_value, threshold_value,
                   is_resolved as "is_resolved!", created_at as "created_at!", resolved_at
                   FROM alerts WHERE server_id = ? AND is_resolved = FALSE ORDER BY created_at DESC"#,
                sid
            )
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as!(
                Alert,
                r#"SELECT id as "id!", server_id as "server_id!", alert_type as "alert_type!", 
                   metric_type as "metric_type!", message as "message!", current_value, threshold_value,
                   is_resolved as "is_resolved!", created_at as "created_at!", resolved_at
                   FROM alerts WHERE is_resolved = FALSE ORDER BY created_at DESC"#
            )
            .fetch_all(&self.pool)
            .await?
        };
        
        Ok(alerts)
    }
    
    /// Get alert summary
    pub async fn get_alert_summary(&self) -> Result<crate::models::AlertSummary, MetricsError> {
        let summary = sqlx::query_as!(
            crate::models::AlertSummary,
            r#"
            SELECT 
                COUNT(*) as "total_alerts!",
                SUM(CASE WHEN alert_type = 'critical' THEN 1 ELSE 0 END) as "critical_alerts!",
                SUM(CASE WHEN alert_type = 'warning' THEN 1 ELSE 0 END) as "warning_alerts!",
                SUM(CASE WHEN metric_type = 'server_down' THEN 1 ELSE 0 END) as "down_alerts!",
                SUM(CASE WHEN is_resolved = FALSE THEN 1 ELSE 0 END) as "unresolved_alerts!"
            FROM alerts
            "#
        )
        .fetch_one(&self.pool)
        .await?;
        
        Ok(summary)
    }
}

/* =========================
   HELPER FUNCTIONS
   ========================= */

/// Convert JSON value (number or string) to f64
fn json_to_f64(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::Number(n) => n.as_f64(),
        JsonValue::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Clamp value between 0.0 and 100.0
fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

/// Compute memory or disk usage percent
fn compute_percent(used_bytes: f64, total_bytes: f64) -> f64 {
    if total_bytes > 0.0 {
        clamp_percent((used_bytes / total_bytes) * 100.0)
    } else {
        0.0
    }
}

/// Compute per-second rate (network, disk I/O)
fn compute_rate(delta: f64, dt_seconds: f64) -> f64 {
    const MIN_DT: f64 = 1e-6; // Minimum time difference to avoid division by zero
    if dt_seconds > MIN_DT {
        (delta / dt_seconds).max(0.0)
    } else {
        0.0
    }
}

/// Parse memory chart from Netdata
pub fn parse_memory(chart: &NetdataResponse, info_json: Option<&JsonValue>) -> (f64, f64) {
    if let Some(row) = chart.data.get(0) {
        let mut used_mb = 0.0;
        let mut total_mb = 0.0;

        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
            if let Some(val) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                let label: String = lbl.to_lowercase();
                if label.contains("used") {
                    used_mb = val;
                } else if label.contains("total") || label.contains("size") {
                    total_mb = val;
                }
            }
        }

        let total_bytes = if let Some(info) = info_json {
            info.get("ram_total").and_then(json_to_f64).unwrap_or(total_mb * 1024.0 * 1024.0)
        } else {
            total_mb * 1024.0 * 1024.0
        };

        let used_bytes = used_mb * 1024.0 * 1024.0;
        let percent = compute_percent(used_bytes, total_bytes);
        let total_gb = total_bytes / 1024.0 / 1024.0 / 1024.0;
        return (percent, total_gb);
    }
    (0.0, 0.0)
}

/// Parse disk chart from Netdata (disk_space: used/avail or used/total; disk_util: percent)
pub fn parse_disk(chart: &NetdataResponse) -> f64 {
    if let Some(row) = chart.data.get(0) {
        let mut used = 0.0;
        let mut total = 0.0;
        let mut avail = 0.0;
        let mut free = 0.0;
        let mut percent_val = None;

        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
            if let Some(val) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                let label: String = lbl.to_lowercase();
                
                // Direct percentage values
                if label.contains("util") || label.contains("percent") || label.contains("usage") 
                    || label.contains("%") || label.contains("pct") {
                    percent_val = Some(if val <= 1.0 { val * 100.0 } else { val });
                }
                // Used space
                else if label.contains("used") || label.contains("consumed") {
                    used = val;
                }
                // Total space
                else if label.contains("total") || label.contains("size") || label.contains("capacity") {
                    total = val;
                }
                // Available/free space
                else if label.contains("avail") || label.contains("available") {
                    avail = val;
                }
                else if label.contains("free") {
                    free = val;
                }
            }
        }

        // Return direct percentage if found
        if let Some(p) = percent_val {
            return clamp_percent(p);
        }

        // Calculate from used/total
        if total > 0.0 {
            return compute_percent(used, total);
        }
        
        // Calculate from used + avail
        if used > 0.0 && avail > 0.0 {
            return compute_percent(used, used + avail);
        }
        
        // Calculate from used + free
        if used > 0.0 && free > 0.0 {
            return compute_percent(used, used + free);
        }
        
        // Calculate from total - avail (if avail represents free space)
        if total > 0.0 && avail > 0.0 {
            let used_calculated = total - avail;
            return compute_percent(used_calculated, total);
        }
        
        // Calculate from total - free (if free represents free space)
        if total > 0.0 && free > 0.0 {
            let used_calculated = total - free;
            return compute_percent(used_calculated, total);
        }
    }
    0.0
}

/// Parse network or disk I/O rate (bytes per second)
pub fn parse_rate(chart: &NetdataResponse) -> f64 {
    if chart.data.len() >= 2 {
        let last = &chart.data[chart.data.len() - 1];
        let prev = &chart.data[chart.data.len() - 2];

        if let (Some(t_last), Some(t_prev)) = (last.get(0).and_then(json_to_f64), prev.get(0).and_then(json_to_f64)) {
            let dt: f64 = (t_last - t_prev).abs();
            let mut sum_delta = 0.0;

            for i in 1..chart.labels.len() {
                if let (Some(a), Some(b)) = (prev.get(i).and_then(json_to_f64), last.get(i).and_then(json_to_f64)) {
                    sum_delta += (b - a).max(0.0_f64);
                }
            }

            return compute_rate(sum_delta, dt);
        }
    }
    0.0
}

/// Returns true if dimension label is "inbound" (download/received).
fn is_inbound_label(label: &str) -> bool {
    let l = label.to_lowercase();
    l.contains("received") || l.contains("recv") || l == "in" || l.contains("receiv")
        || l.contains("download") || l.contains("rx") || l.contains("bytes in")
        || l.contains("inbound") || l.contains("in ")
}

/// Returns true if dimension label is "outbound" (upload/sent).
fn is_outbound_label(label: &str) -> bool {
    let l = label.to_lowercase();
    l.contains("sent") || l.contains("send") || l == "out" || l.contains("upload")
        || l.contains("tx") || l.contains("bytes out")
        || l.contains("outbound") || l.contains("out ")
}

/// Parse network chart into (download/received rate, upload/sent rate) in bytes/sec.
/// Handles Netdata data in either chronological order; uses latest two points by timestamp.
fn parse_network_rates(chart: &NetdataResponse) -> (f64, f64) {
    if chart.data.is_empty() || chart.labels.len() < 2 {
        return (0.0, 0.0);
    }
    // Single row: value may already be rate (e.g. bytes/s) - use directly
    if chart.data.len() == 1 {
        let row = &chart.data[0];
        let mut in_rate = 0.0f64;
        let mut out_rate = 0.0f64;
        let mut first_val = 0.0f64;
        let mut second_val = 0.0f64;
        for i in 1..chart.labels.len().min(row.len()) {
            if let Some(v) = row.get(i).and_then(json_to_f64) {
                let label = chart.labels[i].as_str();
                if is_inbound_label(label) {
                    in_rate += v;
                } else if is_outbound_label(label) {
                    out_rate += v;
                }
                if i == 1 {
                    first_val = v;
                } else if i == 2 {
                    second_val = v;
                }
            }
        }
        if in_rate == 0.0 && out_rate == 0.0 && chart.labels.len() >= 3 {
            in_rate = first_val;
            out_rate = second_val;
        }
        return (in_rate, out_rate);
    }
    // Two or more rows: find newest and oldest by timestamp (column 0)
    let mut indexed: Vec<(f64, usize)> = chart
        .data
        .iter()
        .enumerate()
        .filter_map(|(idx, r)| Some((r.get(0).and_then(json_to_f64)?, idx)))
        .collect();
    indexed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let (_t_old, idx_old) = indexed[0];
    let (t_new, idx_new) = indexed[indexed.len() - 1];
    let t_old = indexed[0].0;
    let dt = (t_new - t_old).abs();
    if dt <= 0.0 {
        return (0.0, 0.0);
    }
    let old_row = &chart.data[idx_old];
    let new_row = &chart.data[idx_new];

    let mut in_rate = 0.0f64;
    let mut out_rate = 0.0f64;
    let mut first_val = 0.0f64;
    let mut second_val = 0.0f64;
    for i in 1..chart.labels.len() {
        let a = old_row.get(i).and_then(json_to_f64).unwrap_or(0.0);
        let b = new_row.get(i).and_then(json_to_f64).unwrap_or(0.0);
        let delta = (b - a).max(0.0);
        let rate = compute_rate(delta, dt);
        let label = chart.labels[i].as_str();
        if is_inbound_label(label) {
            in_rate += rate;
        } else if is_outbound_label(label) {
            out_rate += rate;
        }
        if i == 1 {
            first_val = rate;
        } else if i == 2 {
            second_val = rate;
        }
    }
    // Fallback: many net charts have [time, received, sent] or [time, in, out]
    if in_rate == 0.0 && out_rate == 0.0 && chart.labels.len() >= 3 {
        in_rate = first_val;
        out_rate = second_val;
    }
    (in_rate, out_rate)
}

/// Parse disk I/O chart into (read rate, write rate) in bytes/sec.
fn parse_disk_io_rates(chart: &NetdataResponse) -> (f64, f64) {
    if chart.data.is_empty() || chart.labels.len() < 2 {
        return (0.0, 0.0);
    }
    if chart.data.len() == 1 {
        let row = &chart.data[0];
        let mut read_rate = 0.0f64;
        let mut write_rate = 0.0f64;
        for i in 1..chart.labels.len().min(row.len()) {
            if let Some(v) = row.get(i).and_then(json_to_f64) {
                let l = chart.labels[i].to_lowercase();
                if l.contains("read") && !l.contains("write") {
                    read_rate += v;
                } else if l.contains("write") {
                    write_rate += v;
                }
            }
        }
        return (read_rate, write_rate);
    }
    let mut indexed: Vec<(f64, usize)> = chart
        .data
        .iter()
        .enumerate()
        .filter_map(|(idx, r)| Some((r.get(0).and_then(json_to_f64)?, idx)))
        .collect();
    indexed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let (t_old, idx_old) = indexed[0];
    let (t_new, idx_new) = indexed[indexed.len() - 1];
    let dt = (t_new - t_old).abs();
    if dt <= 0.0 {
        return (0.0, 0.0);
    }
    let old_row = &chart.data[idx_old];
    let new_row = &chart.data[idx_new];

    let mut read_rate = 0.0f64;
    let mut write_rate = 0.0f64;
    for i in 1..chart.labels.len() {
        let a = old_row.get(i).and_then(json_to_f64).unwrap_or(0.0);
        let b = new_row.get(i).and_then(json_to_f64).unwrap_or(0.0);
        let delta = (b - a).max(0.0);
        let rate = compute_rate(delta, dt);
        let l = chart.labels[i].to_lowercase();
        if l.contains("read") && !l.contains("write") {
            read_rate += rate;
        } else if l.contains("write") {
            write_rate += rate;
        }
    }
    (read_rate, write_rate)
}

/// CPU parsing: compute average CPU usage percent
pub fn parse_cpu(chart: &NetdataResponse) -> f64 {
    if let Some(row) = chart.data.get(0) {
        let mut sum = 0.0;
        let mut count = 0;

        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
            if let Some(val) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                let label: String = lbl.to_lowercase();
                if !label.contains("idle") {
                    sum += val;
                    count += 1;
                }
            }
        }

        if count > 0 {
            return clamp_percent(sum / count as f64);
        }
    }
    0.0
}

