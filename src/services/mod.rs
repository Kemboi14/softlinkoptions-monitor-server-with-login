use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use serde_json::Value as JsonValue;
use sqlx::SqlitePool;
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

use crate::models::{ServerStats, NetdataResponse};

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
                .timeout(Duration::from_secs(10))
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

        // CPU: try to derive using 'idle' dimension where possible
        let cpu_candidates = ["system.cpu", "system.cpu_all", "system.cpu0"];
        let mut cpu_usage = None;
        for c in cpu_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, c).await {
                cpu_usage = Some(parse_cpu(&chart));
                if cpu_usage.is_some() { break; }
            }
        }

        // Fetch info JSON once for authoritative totals (ram, disk) and uptime
        let info_json = get_json(&self.client, &base, "api/v1/info").await.ok();

        // Memory: find used and total
        let mem_candidates = ["system.ram", "system.ram_size", "system.memory"];
        let mut memory_usage: f64 = 0.0;
        let mut memory_total_gb = 0.0;
        for c in mem_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, c).await {
                let (usage, total) = parse_memory(&chart, info_json.as_ref());
                memory_usage = usage;
                memory_total_gb = total;
                break;
            }
        }

        // Disk: try to read root mount usage (Netdata: disk_space._ for root, or disk_space./)
        let disk_candidates = [
            "disk_space._",
            "disk_space._root",
            "disk_space./",
            "disk_space./root",
            "disk_space._root_",
            "system.disk",
            "disk.space._",
            "disk_util._",
            "disk_util._root",
            "disk_util./",
        ];
        let mut disk_usage_percent: f64 = 0.0;
        for c in disk_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, c).await {
                let pct = parse_disk(&chart);
                if pct > 0.0 && pct <= 100.0 {
                    disk_usage_percent = pct;
                    break;
                }
            }
        }

        // Uptime: first try common uptime charts, then /api/v1/info as fallback
        let mut uptime_secs = 0i64;
        let uptime_candidates = ["system.uptime", "uptime", "system.uptime_seconds"];
        for uc in uptime_candidates {
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
            if uptime_secs > 0 { break; }
        }

        // Load average
        let mut load_avg = 0.0f64;
        if let Ok(chart) = get_chart(&self.client, &base, "system.load").await {
            if let Some(row) = chart.data.get(0) {
                for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                    if lbl.to_lowercase().contains("load1") || lbl.to_lowercase().contains("1m") {
                        if let Some(v) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                            load_avg = v;
                            break;
                        }
                    }
                }
            }
        }

        // Logged users
        let mut logged_users = 0i64;
        if let Ok(chart) = get_chart(&self.client, &base, "system.users").await {
            if let Some(row) = chart.data.get(0) {
                for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                    if lbl.to_lowercase().contains("users") {
                        if let Some(v) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                            logged_users = v as i64;
                            break;
                        }
                    }
                }
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
            if let Ok(chart) = get_chart(&self.client, &base, net_chart).await {
                let (in_r, out_r) = parse_network_rates(&chart);
                net_in_rate += in_r;
                net_out_rate += out_r;
            }
        }

        // Disk I/O rates: read and write separately
        let (mut disk_read_rate, mut disk_write_rate) = (0.0f64, 0.0f64);
        let disk_io_candidates = ["disk_io._total", "disk_io.sda", "disk_io.nvme0n1", "disk.io", "system.io"];
        for disk_io_chart in disk_io_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, disk_io_chart).await {
                let (r, w) = parse_disk_io_rates(&chart);
                if r > 0.0 || w > 0.0 {
                    disk_read_rate = r;
                    disk_write_rate = w;
                    break;
                }
            }
        }

        let cpu_final = cpu_usage.unwrap_or(0.0).clamp(0.0, 100.0);
        let mem_final = memory_usage.clamp(0.0, 100.0);
        let disk_final = disk_usage_percent.clamp(0.0, 100.0);
        
        info!("Final metrics for {}: CPU={:.1}%, MEM={:.1}%, DISK={:.1}%", 
               server_ip, cpu_final, mem_final, disk_final);

        Ok(ServerStats {
            id: Uuid::new_v4().to_string(),
            server_id: String::new(),
            cpu_usage: cpu_final,
            memory_usage: mem_final,
            memory_total: memory_total_gb,
            disk_usage: disk_final,
            load_avg,
            logged_users,
            network_in: 0.0,
            network_out: 0.0,
            network_in_rate: net_in_rate,
            network_out_rate: net_out_rate,
            disk_read_rate: disk_read_rate,
            disk_write_rate: disk_write_rate,
            uptime: uptime_secs,
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
    if dt_seconds > 0.0 {
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
        let mut percent_val = None;

        for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
            if let Some(val) = row.get(i).and_then(|val: &JsonValue| json_to_f64(val)) {
                let label: String = lbl.to_lowercase();
                if label.contains("util") || label.contains("percent") || label.contains("usage") {
                    percent_val = Some(if val <= 1.0 { val * 100.0 } else { val });
                } else if label.contains("used") {
                    used = val;
                } else if label.contains("total") || label.contains("size") {
                    total = val;
                } else if label.contains("avail") || label.contains("free") {
                    avail = val;
                }
            }
        }

        if let Some(p) = percent_val {
            return clamp_percent(p);
        }

        if total > 0.0 {
            return compute_percent(used, total);
        }
        if used > 0.0 && avail > 0.0 {
            return compute_percent(used, used + avail);
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

