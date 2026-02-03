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
    pub uptime: i64,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewServer {
    pub name: String,
    pub ip_address: String,
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
    pub period: String,
}
