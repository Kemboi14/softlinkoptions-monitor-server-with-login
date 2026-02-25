use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use chrono::NaiveDateTime;

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub ip_address: String,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct ServerStats {
    pub id: String,
    pub server_id: String,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub memory_total: f64,
    pub disk_usage: f64,
    pub load_avg: f64,
    pub logged_users: i64,
    pub network_in: f64,
    pub network_out: f64,
    // per-second rates (bytes/sec)
    pub network_in_rate: f64,
    pub network_out_rate: f64,
    // disk IO rates (bytes/sec)
    pub disk_read_rate: f64,
    pub disk_write_rate: f64,
    pub uptime: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NetdataResponse {
    pub labels: Vec<String>,
    pub data: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerWithStats {
    pub server: Server,
    pub latest_stats: Option<ServerStats>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AverageStats {
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub disk_usage: f64,
    pub load_avg: f64,
    pub network_in: f64,
    pub network_out: f64,
    /// Average network in rate (bytes/sec); display as KB/s with /1024
    pub network_in_rate: f64,
    /// Average network out rate (bytes/sec); display as KB/s with /1024
    pub network_out_rate: f64,
    pub period: String,
}

#[derive(Debug, Serialize, Deserialize, FromRow, Clone)]
pub struct Alert {
    pub id: String,
    pub server_id: String,
    pub alert_type: String, // 'critical', 'warning', 'down'
    pub metric_type: String, // 'cpu', 'memory', 'disk', 'server_down'
    pub message: String,
    pub current_value: Option<f64>,
    pub threshold_value: Option<f64>,
    pub is_resolved: bool,
    pub created_at: NaiveDateTime,
    pub resolved_at: Option<NaiveDateTime>,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct AlertPreference {
    pub id: String,
    pub user_id: String,
    pub cpu_threshold: f64,
    pub memory_threshold: f64,
    pub disk_threshold: f64,
    pub load_threshold: f64,
    pub enable_notifications: bool,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct AlertSummary {
    pub total_alerts: i64,
    pub critical_alerts: i64,
    pub warning_alerts: i64,
    pub down_alerts: i64,
    pub unresolved_alerts: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AlertWithServer {
    pub alert: Alert,
    pub server: Server,
}
