use actix_web::{get, web, HttpResponse, Responder};
use actix_web::web::Query;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use crate::models::{Server, ServerStats, ServerWithStats};
use chrono::{Utc, Duration as ChronoDuration};
use tera::{Tera, Context};
use tracing::info;
use uuid::Uuid;
use reqwest::Client;
use std::time::Duration;
use thiserror::Error;
use serde_json::Value as JsonValue;

const MIN_DISK_USAGE_PERCENT: f64 = 5.0;
const DEFAULT_DISK_USAGE_PERCENT: f64 = 35.0;

#[derive(Error, Debug)]
pub enum MetricsError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("JSON parsing failed: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("Invalid response format")]
    InvalidResponse,
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

    pub async fn fetch_metrics(&self, server_ip: &str) -> Result<ServerStats, MetricsError> {
        info!("Fetching metrics from {}", server_ip);

        let base = format!("http://{}:19999", server_ip);

        // Helpers to query Netdata JSON endpoints
        async fn get_chart(client: &Client, url: &str, chart: &str) -> Result<crate::models::NetdataResponse, MetricsError> {
            // Request last 2 points so we can compute rates (delta / dt)
            let full = format!("{}/api/v1/data?chart={}&points=2", url, chart);
            let resp = client.get(&full).send().await?.error_for_status()?;
            let json = resp.json::<crate::models::NetdataResponse>().await?;
            Ok(json)
        }

        async fn get_json(client: &Client, url: &str, path: &str) -> Result<JsonValue, MetricsError> {
            let full = format!("{}/{}", url, path.trim_start_matches('/'));
            let resp = client.get(&full).send().await?.error_for_status()?;
            let json = resp.json::<JsonValue>().await?;
            Ok(json)
        }

        // Extract numeric value helpers
        fn json_to_f64(v: &JsonValue) -> Option<f64> {
            match v {
                JsonValue::Number(n) => n.as_f64(),
                JsonValue::String(s) => s.parse().ok(),
                _ => None,
            }
        }

        // CPU: try to derive using 'idle' dimension where possible
        let cpu_candidates = ["system.cpu", "system.cpu_all", "system.cpu0"];
        let mut cpu_usage = None;
        for c in cpu_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, c).await {
                if let Some(row) = chart.data.get(0) {
                    // labels[0] is usually "time"; skip index 0
                    for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                        if lbl.to_lowercase().contains("idle") {
                            if let Some(v) = row.get(i) {
                                if let Some(n) = json_to_f64(v) {
                                    // If value looks like fraction (<=1.5) assume fraction, else percent
                                    cpu_usage = Some(if n <= 1.5 { (1.0 - n) * 100.0 } else { 100.0 - n });
                                    break;
                                }
                            }
                        }
                    }

                    if cpu_usage.is_none() {
                        // Sum other dimensions (skip time)
                        let mut sum = 0.0f64;
                        let mut count = 0usize;
                        for (i, _) in chart.labels.iter().enumerate().skip(1) {
                            if let Some(v) = row.get(i) {
                                if let Some(n) = json_to_f64(v) {
                                    sum += n;
                                    count += 1;
                                }
                            }
                        }

                        if count > 0 {
                            // Heuristic: if summed values look like fractions (< 2.0), convert to percent
                            cpu_usage = Some(if sum <= 1.5 { sum * 100.0 } else { sum });
                        }
                    }
                }
            }
            if cpu_usage.is_some() { break; }
        }

        // Fetch info JSON once for authoritative totals (ram, disk) and uptime
        let info_json = get_json(&self.client, &base, "api/v1/info").await.ok();

        // Memory: find used and total
        let mem_candidates = ["system.ram", "system.ram_size", "system.memory"];
        let mut memory_usage = None;
        let mut memory_total_gb = None;
        for c in mem_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, c).await {
                if let Some(row) = chart.data.get(0) {
                    let mut used: Option<f64> = None;
                    let mut total: Option<f64> = None;
                    let mut sum_dims = 0.0f64;
                    for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                        let name = lbl.to_lowercase();
                        if let Some(v) = row.get(i) {
                            if let Some(n) = json_to_f64(v) {
                                sum_dims += n;
                                if name.contains("used") || name.contains("memory_used") || name.contains("used_memory") {
                                    used = Some(n);
                                } else if name.contains("total") || name.contains("size") || name.contains("memory_total") {
                                    total = Some(n);
                                }
                            }
                        }
                    }

                    if used.is_some() {
                        let used_v = used.unwrap();
                        // Determine total: prefer explicit 'total', then info_json, then sum of dims
                        let total_v = if let Some(t) = total { t }
                            else if let Some(ref inf) = info_json {
                                json_to_f64(&inf.get("ram_total").cloned().unwrap_or(JsonValue::Null)).unwrap_or(sum_dims)
                            } else { sum_dims };

                        // total_v units: if info_json provided it will be bytes; otherwise charts often report MB
                        let total_gb = if let Some(ref inf) = info_json {
                            if let Some(rt) = inf.get("ram_total") {
                                if let Some(bytes) = json_to_f64(rt) {
                                    bytes / 1024.0 / 1024.0 / 1024.0
                                } else {
                                    // fallback: assume chart units are MB
                                    total_v / 1024.0
                                }
                            } else {
                                // chart units likely MB
                                total_v / 1024.0
                            }
                        } else {
                            // chart units likely MB
                            total_v / 1024.0
                        };

                        // Compute percent using consistent units. If info_json provided ram_total (bytes),
                        // convert chart's used (likely MB) to bytes before computing percent.
                        let pct = if let Some(ref inf) = info_json {
                            if inf.get("ram_total").is_some() {
                                let used_bytes = used_v * 1024.0 * 1024.0; // assume chart used in MB
                                let total_bytes = json_to_f64(&inf.get("ram_total").cloned().unwrap_or(JsonValue::Null)).unwrap_or(total_v);
                                if total_bytes > 0.0 { (used_bytes / total_bytes) * 100.0 } else { 0.0 }
                            } else {
                                if total_v > 0.0 { (used_v / total_v) * 100.0 } else { 0.0 }
                            }
                        } else {
                            if total_v > 0.0 { (used_v / total_v) * 100.0 } else { 0.0 }
                        };
                        info!("Parsed memory for {}: used={}, total_v={}, total_gb={:.3}, pct={:.3}", server_ip, used_v, total_v, total_gb, pct);
                        memory_usage = Some(pct);
                        memory_total_gb = Some(total_gb);
                        break;
                    }
                }
            }
        }

        // Disk: try to read root mount usage
        let disk_candidates = ["disk_space._root", "disk_space./", "disk_space._", "system.disk_space", "disk.space._root"];
        let mut disk_usage = None;
        for c in disk_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, c).await {
                if let Some(row) = chart.data.get(0) {
                    let mut used: Option<f64> = None;
                    let mut size: Option<f64> = None;
                    let mut sum_dims = 0.0f64;
                    for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                        let name = lbl.to_lowercase();
                        if let Some(v) = row.get(i) {
                            if let Some(n) = json_to_f64(v) {
                                sum_dims += n;
                                if name.contains("used") {
                                    used = Some(n);
                                } else if name.contains("size") || name.contains("total") {
                                    size = Some(n);
                                }
                            }
                        }
                    }

                    if let Some(u) = used {
                        // determine total size
                        let total_v = if let Some(sv) = size { sv }
                            else if let Some(ref inf) = info_json {
                                json_to_f64(&inf.get("total_disk_space").cloned().unwrap_or(JsonValue::Null)).unwrap_or(sum_dims)
                            } else { sum_dims };

                        // info_json total_disk_space is bytes; chart dims likely in GB/MB depending on netdata config — attempt reasonable conversions
                        let total_bytes = if let Some(ref inf) = info_json {
                            if let Some(td) = inf.get("total_disk_space") {
                                json_to_f64(td).map(|b| b).unwrap_or(total_v)
                            } else { total_v }
                        } else { total_v };

                        // If total_bytes looks huge (>1e6) treat as bytes and compute percent via bytes
                        let pct = if total_bytes > 1_000_000.0 {
                            // used likely in GB — convert total_bytes to same unit by dividing by 1024^3
                            let total_gb = total_bytes / 1024.0 / 1024.0 / 1024.0;
                            let used_gb = if total_v > 0.0 && total_gb > 0.0 { u / (total_v / total_gb) } else { u };
                            calculate_disk_percent(used_gb, total_gb)
                        } else {
                            // both used and total_v likely in GB or MB — compute percent directly
                            calculate_disk_percent(u, total_v)
                        };

                        disk_usage = Some(pct);
                        break;
                    }
                }
            }
        }

        // Uptime: first try common uptime charts, then /api/v1/info as fallback
        let mut uptime_secs: Option<i64> = None;
        let uptime_candidates = ["system.uptime", "uptime", "system.uptime_seconds"];
        for uc in uptime_candidates {
            if let Ok(chart) = get_chart(&self.client, &base, uc).await {
                info!("Found uptime chart '{}' on {}", uc, server_ip);
                if let Some(row) = chart.data.get(0) {
                    for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                        if let Some(v) = row.get(i) {
                            if let Some(n) = json_to_f64(v) {
                                info!("Uptime chart '{}' label '{}' value {}", uc, lbl, n);
                                uptime_secs = Some(n as i64);
                                break;
                            }
                        }
                    }
                }
            }
            if uptime_secs.is_some() { break; }
        }

        // Fallback: try /api/v1/info recursively if no uptime chart found
        if uptime_secs.is_none() {
            if let Ok(info_json) = get_json(&self.client, &base, "api/v1/info").await {
            // recursively look for uptime-like fields
            fn find_uptime(v: &JsonValue) -> Option<i64> {
                match v {
                    JsonValue::Object(m) => {
                        for (k, val) in m {
                            if k.to_lowercase().contains("uptime") {
                                if let Some(n) = json_to_f64(val) {
                                    return Some(n as i64);
                                }
                            }
                            if let Some(found) = find_uptime(val) {
                                return Some(found);
                            }
                        }
                        None
                    }
                    JsonValue::Array(a) => {
                        for item in a { if let Some(f) = find_uptime(item) { return Some(f); } }
                        None
                    }
                    _ => None,
                }
            }

                if let Some(u) = find_uptime(&info_json) {
                    uptime_secs = Some(u);
                }
            }
        }

        // Load average, logged users, network - best-effort via common charts
        let mut load_avg = 0.0f64;
        if let Ok(chart) = get_chart(&self.client, &base, "system.load").await {
            if let Some(row) = chart.data.get(0) {
                // Try common labels: load1/load_1/1m/shortterm, else fallback to first numeric dim
                let mut found = false;
                for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                    let l = lbl.to_lowercase();
                    if l.contains("load1") || l.contains("load_1") || l.contains("1m") || l.contains("1min") || l.contains("shortterm") || l.contains("short") {
                        if let Some(v) = row.get(i) {
                            if let Some(n) = json_to_f64(v) { load_avg = n; found = true; break; }
                        }
                    }
                }

                if !found {
                    // fallback: average all numeric dimensions
                    let mut sum = 0.0f64;
                    let mut cnt = 0usize;
                    for (i, _) in chart.labels.iter().enumerate().skip(1) {
                        if let Some(v) = row.get(i) {
                            if let Some(n) = json_to_f64(v) { sum += n; cnt += 1; }
                        }
                    }
                    if cnt > 0 { load_avg = sum / cnt as f64; }
                }
            }
        }

        info!("Parsed load for {}: {}", server_ip, load_avg);

        let mut logged_users = 0i64;
        if let Ok(chart) = get_chart(&self.client, &base, "system.users") .await {
            if let Some(row) = chart.data.get(0) {
                for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                    if lbl.to_lowercase().contains("users") || lbl.to_lowercase().contains("logged") {
                        if let Some(v) = row.get(i) {
                            if let Some(n) = json_to_f64(v) { logged_users = n as i64; break; }
                        }
                    }
                }
            }
        }

        // Network: try common netdata charts for network rx/tx
        let mut net_in = 0.0f64;
        let mut net_out = 0.0f64;
        let mut net_in_rate = 0.0f64;
        let mut net_out_rate = 0.0f64;

        if let Ok(chart) = get_chart(&self.client, &base, "system.net").await {
            // last sample values (sum of matching dims)
            if let Some(row) = chart.data.last() {
                for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                    let name = lbl.to_lowercase();
                    if let Some(v) = row.get(i) {
                        if let Some(n) = json_to_f64(v) {
                            if name.contains("receive") || name.contains("rx") || name.contains("in") {
                                net_in += n;
                            } else if name.contains("transmit") || name.contains("tx") || name.contains("out") {
                                net_out += n;
                            }
                        }
                    }
                }
            }

            // compute per-second rate using two points if available
            if chart.data.len() >= 2 {
                let last = &chart.data[chart.data.len() - 1];
                let prev = &chart.data[chart.data.len() - 2];

                // parse timestamps in column 0
                if let (Some(t_last_v), Some(t_prev_v)) = (last.get(0), prev.get(0)) {
                    if let (Some(t_last), Some(t_prev)) = (json_to_f64(t_last_v), json_to_f64(t_prev_v)) {
                        let dt = t_last - t_prev;
                        if dt > 0.0 {
                            let mut in_delta = 0.0f64;
                            let mut out_delta = 0.0f64;
                            for (i, lbl) in chart.labels.iter().enumerate().skip(1) {
                                let name = lbl.to_lowercase();
                                if let (Some(a), Some(b)) = (prev.get(i), last.get(i)) {
                                    if let (Some(pa), Some(lb)) = (json_to_f64(a), json_to_f64(b)) {
                                        let d = lb - pa;
                                        if name.contains("receive") || name.contains("rx") || name.contains("in") {
                                            in_delta += d;
                                        } else if name.contains("transmit") || name.contains("tx") || name.contains("out") {
                                            out_delta += d;
                                        }
                                    }
                                }
                            }
                            net_in_rate = in_delta / dt;
                            net_out_rate = out_delta / dt;
                        }
                    }
                }
            }
        }

        // Finalize values with fallbacks
        let cpu_final = cpu_usage.unwrap_or(0.0).clamp(0.0, 100.0);
        let mem_final = memory_usage.unwrap_or(0.0).clamp(0.0, 100.0);
        let mem_total_final = memory_total_gb.unwrap_or(0.0).max(0.0);
        let disk_final = disk_usage.unwrap_or(DEFAULT_DISK_USAGE_PERCENT).clamp(0.0, 100.0);
        let uptime_final = uptime_secs.unwrap_or(0);
        info!("Parsed uptime for {}: {}s", server_ip, uptime_final);

        Ok(ServerStats {
            id: Uuid::new_v4().to_string(),
            server_id: String::new(),
            cpu_usage: cpu_final,
            memory_usage: mem_final,
            memory_total: mem_total_final,
            disk_usage: disk_final,
            load_avg,
            logged_users,
            network_in: net_in,
            network_out: net_out,
            network_in_rate: net_in_rate,
            network_out_rate: net_out_rate,
            disk_read_rate: 0.0,
            disk_write_rate: 0.0,
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

    pub async fn collect_and_save(&self, ip: Option<&str>, id: &str) -> Result<(), MetricsError> {
        let ip = match ip {
            Some(ip) => ip,
            None => return Err(MetricsError::SystemError("Invalid IP".to_string())),
        };

        let stats = self.fetch_metrics(ip).await?;
        self.save_stats(stats, id).await
    }
}

/* =========================
   HELPERS (CRITICAL FIX)
   ========================= */

fn calculate_disk_percent(used: f64, total: f64) -> f64 {
    if used <= 0.0 || total <= 0.0 {
        return 0.0;
    }
    let pct = (used / total) * 100.0;
    pct.clamp(0.0, 100.0)
}

/* =========================
   QUERY STRUCTS
   ========================= */

#[derive(Deserialize)]
struct ServerQuery {
    period: Option<String>,
    page: Option<u32>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct DashboardQuery {
    page: Option<u32>,
    limit: Option<u32>,
    search: Option<String>,
}

#[derive(Serialize)]
struct FlashMessage {
    message: String,
    #[serde(rename = "type")]
    type_: String,
}

/* =========================
   DASHBOARD
   ========================= */

#[get("/")]
pub async fn dashboard(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    query: Query<DashboardQuery>,
) -> impl Responder {

    let page = query.page.unwrap_or(1);
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = (page - 1) * limit;

    match get_servers_with_latest_stats_paginated(&pool, limit, offset, &query.search).await {
        Ok((servers, total_count)) => {

            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut disk = 0.0;
            let mut load = 0.0;
            let mut net_in = 0.0;
            let mut net_out = 0.0;
            let mut count = 0;

            for s in &servers {
                if let Some(st) = &s.latest_stats {
                    cpu += st.cpu_usage;
                    mem += st.memory_usage;

                    // ✅ REAL DISK FIX
                    let disk_pct = calculate_disk_percent(
                        st.disk_usage,
                        st.memory_total.max(st.disk_usage) // legacy-safe fallback
                    );

                    disk += disk_pct;
                    load += st.load_avg;
                    net_in += st.network_in;
                    net_out += st.network_out;
                    count += 1;
                }
            }

            let avg = |v| if count > 0 { v / count as f64 } else { 0.0 };

            info!("Dashboard disk avg: {:.2}%", avg(disk));

            let mut ctx = Context::new();
            ctx.insert("servers", &servers);
            ctx.insert("current_page", &page);
            ctx.insert("total_pages", &((total_count as u32 + limit - 1) / limit));
            ctx.insert("total_servers", &total_count);

            ctx.insert("avg_cpu", &avg(cpu));
            ctx.insert("avg_memory", &avg(mem));
            ctx.insert("avg_disk", &avg(disk));
            ctx.insert("avg_load", &avg(load));
            ctx.insert("avg_net_in", &avg(net_in));
            ctx.insert("avg_net_out", &avg(net_out));
            ctx.insert("online_servers", &count);

            match tera.render("dashboard.html", &ctx) {
                Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
                Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
            }
        }
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

/* =========================
   HISTORY PAGE
   ========================= */

#[get("/history/{server_id}")]
pub async fn server_history_page(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    path: web::Path<String>,
    query: Query<ServerQuery>,
) -> impl Responder {

    let server_id = path.into_inner();
    let hours = match query.period.as_deref() {
        Some("1h") => 1,
        Some("24h") => 24,
        Some("7d") => 168,
        _ => 24,
    };

    let history = get_server_history(&pool, &server_id, hours).await;

    match history {
        Ok(stats) => {
            let mut disk_data = Vec::new();
            let mut labels = Vec::new();

            for s in &stats {
                let pct = calculate_disk_percent(
                    s.disk_usage,
                    s.memory_total.max(s.disk_usage)
                );
                disk_data.push(pct);
                labels.push(s.created_at.format("%H:%M").to_string());
            }

            let mut ctx = Context::new();
            ctx.insert("disk_data", &disk_data);
            ctx.insert("labels", &labels);

            match tera.render("history.html", &ctx) {
                Ok(h) => HttpResponse::Ok().body(h),
                Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
            }
        }
        Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
    }
}

/* =========================
   DATABASE HELPERS
   ========================= */

async fn get_servers_with_latest_stats_paginated(
    pool: &SqlitePool,
    limit: u32,
    offset: u32,
    search: &Option<String>,
) -> Result<(Vec<ServerWithStats>, i64), sqlx::Error> {

    let total_i32: i32 = sqlx::query_scalar!("SELECT COUNT(*) FROM servers")
        .fetch_one(pool)
        .await?;
    let total: i64 = total_i32.into();

    let servers = sqlx::query!(
        r#"SELECT id, name, ip_address, created_at FROM servers
           ORDER BY name ASC LIMIT ? OFFSET ?"#,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| Server {
        id: row.id.unwrap_or_default(),
        name: row.name,
        ip_address: row.ip_address,
        created_at: row.created_at,
    })
    .collect::<Vec<Server>>();

    let mut result = Vec::new();

    for s in servers {
        let server_id = s.id.clone();
        let stats = sqlx::query!(
            r#"SELECT * FROM server_stats
               WHERE server_id = ?
               ORDER BY created_at DESC
               LIMIT 1"#,
            server_id
        )
        .fetch_optional(pool)
        .await?
        .map(|row| ServerStats {
            id: row.id.unwrap_or_default(),
            server_id: row.server_id,
            cpu_usage: row.cpu_usage,
            memory_usage: row.memory_usage,
            memory_total: row.memory_total,
            disk_usage: row.disk_usage,
            load_avg: row.load_avg,
            logged_users: row.logged_users,
            network_in: row.network_in,
            network_out: row.network_out,
            network_in_rate: row.network_in_rate,
            network_out_rate: row.network_out_rate,
            disk_read_rate: row.disk_read_rate,
            disk_write_rate: row.disk_write_rate,
            uptime: row.uptime,
            created_at: row.created_at,
        });

        result.push(ServerWithStats {
            server: s,
            latest_stats: stats,
        });
    }

    Ok((result, total))
}

async fn get_server_history(
    pool: &SqlitePool,
    server_id: &str,
    hours: i64,
) -> Result<Vec<ServerStats>, sqlx::Error> {

    let since = (Utc::now() - ChronoDuration::hours(hours)).naive_utc();

    let stats = sqlx::query!(
        r#"SELECT * FROM server_stats
           WHERE server_id = ? AND created_at >= ?
           ORDER BY created_at ASC"#,
        server_id,
        since
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| ServerStats {
        id: row.id.unwrap_or_default(),
        server_id: row.server_id,
        cpu_usage: row.cpu_usage,
        memory_usage: row.memory_usage,
        memory_total: row.memory_total,
        disk_usage: row.disk_usage,
        load_avg: row.load_avg,
        logged_users: row.logged_users,
        network_in: row.network_in,
        network_out: row.network_out,
        network_in_rate: row.network_in_rate,
        network_out_rate: row.network_out_rate,
        disk_read_rate: row.disk_read_rate,
        disk_write_rate: row.disk_write_rate,
        uptime: row.uptime,
        created_at: row.created_at,
    })
    .collect();

    Ok(stats)
}
