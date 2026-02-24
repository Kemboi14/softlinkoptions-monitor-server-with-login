use actix_web::{get, post, delete, web, HttpResponse, Responder};
use actix_web::web::Query;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use crate::models::{Server, ServerStats, ServerWithStats, AverageStats};
use chrono::{Utc, Duration};
use tera::{Tera, Context};
use uuid::Uuid;
use tracing::error;

#[derive(Deserialize)]
struct LoginCredentials {
    username: String,
    password: String,
}

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

#[get("/")]
pub async fn dashboard(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    query: Query<DashboardQuery>,
    req: actix_web::HttpRequest,
) -> impl Responder {
    // Simple authentication check - in production, use proper session management
    let auth_header = req.headers().get("cookie");
    let is_authenticated = auth_header
        .and_then(|cookie| cookie.to_str().ok())
        .map(|cookie_str| cookie_str.contains("authenticated=true"))
        .unwrap_or(false);
    
    if !is_authenticated {
        return HttpResponse::Found()
            .append_header(("Location", "/login"))
            .finish();
    }
    
    let page = query.page.unwrap_or(1);
    let limit = query.limit.unwrap_or(50).min(100); // Max 100 per page
    let offset = (page - 1) * limit;
    
    // Check for flash message in cookies
    let flash_message = req.headers().get("cookie")
        .and_then(|cookie| cookie.to_str().ok())
        .and_then(|cookie_str| {
            cookie_str.split(';')
                .find(|c| c.trim().starts_with("flash_message="))
                .and_then(|c| {
                    c.trim().strip_prefix("flash_message=")
                        .map(|msg| {
                            let parts: Vec<&str> = msg.splitn(2, ';').collect();
                            FlashMessage {
                                message: parts.get(0).unwrap_or(&msg).to_string(),
                                type_: if parts.get(0).unwrap_or(&msg).contains("success") { "success".to_string() }
                                       else if parts.get(0).unwrap_or(&msg).contains("fail") { "danger".to_string() }
                                       else { "info".to_string() },
                            }
                        })
                })
        });
    
    match get_servers_with_latest_stats_paginated(&pool, limit, offset, &query.search).await {
        Ok((servers, total_count)) => {
            let total_pages = ((total_count as u32 + limit - 1) / limit) as u32;
            
            // Calculate real averages from current servers
            let mut total_cpu = 0.0;
            let mut total_memory = 0.0;
            let mut total_disk = 0.0;
            let mut total_load = 0.0;
            let mut total_net_in_rate = 0.0;
            let mut total_net_out_rate = 0.0;
            let mut server_count = 0;
            
            for server_with_stats in &servers {
                if let Some(stats) = &server_with_stats.latest_stats {
                    total_cpu += stats.cpu_usage;
                    total_memory += stats.memory_usage;
                    total_disk += stats.disk_usage;
                    total_load += stats.load_avg;
                    total_net_in_rate += stats.network_in_rate; // Use rate
                    total_net_out_rate += stats.network_out_rate; // Use rate
                    server_count += 1;
                }
            }
            
            let avg_cpu = if server_count > 0 { total_cpu / server_count as f64 } else { 0.0 };
            let avg_memory = if server_count > 0 { total_memory / server_count as f64 } else { 0.0 };
            let avg_disk = if server_count > 0 { total_disk / server_count as f64 } else { 0.0 };
            let avg_load = if server_count > 0 { total_load / server_count as f64 } else { 0.0 };
            let avg_net_in_rate = if server_count > 0 { (total_net_in_rate / server_count as f64) / 1024.0 } else { 0.0 };
            let avg_net_out_rate = if server_count > 0 { (total_net_out_rate / server_count as f64) / 1024.0 } else { 0.0 };

            let mut context = Context::new();
            context.insert("servers", &servers);
            context.insert("current_page", &page);
            context.insert("total_pages", &total_pages);
            context.insert("total_servers", &total_count);
            context.insert("limit", &limit);
            context.insert("search", &query.search);
            
            // Insert calculated averages
            context.insert("avg_cpu", &avg_cpu);
            context.insert("avg_memory", &avg_memory);
            context.insert("avg_disk", &avg_disk);
            context.insert("avg_load", &avg_load);
            context.insert("avg_net_in", &avg_net_in_rate); // Update context variable
            context.insert("avg_net_out", &avg_net_out_rate); // Update context variable
            context.insert("online_servers", &server_count);
            
            if let Some(flash) = flash_message {
                context.insert("flash_message", &flash);
            }
            
            let mut response = match tera.render("dashboard.html", &context) {
                Ok(rendered) => HttpResponse::Ok().content_type("text/html").body(rendered),
                Err(e) => {
                    error!(err = %e, "Dashboard template render failed");
                    HttpResponse::InternalServerError().body("An error occurred. Please try again.")
                }
            };
            
            // Clear the flash message cookie
            use actix_web::http::header::{HeaderValue, HeaderName};
            response.headers_mut().insert(
                HeaderName::from_static("set-cookie"),
                HeaderValue::from_str("flash_message=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT").unwrap()
            );
            
            response
        }
        Err(e) => {
            error!(err = %e, "Dashboard database error");
            HttpResponse::InternalServerError().body("An error occurred. Please try again.")
        }
    }
}

#[get("/servers")]
pub async fn servers_list(pool: web::Data<SqlitePool>) -> impl Responder {
    match get_all_servers(&pool).await {
        Ok(servers) => HttpResponse::Ok().json(servers),
        Err(e) => {
            error!(err = %e, "List servers error");
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch servers"
            }))
        }
    }
}

#[derive(Deserialize)]
struct NewServerForm {
    name: String,
    ip_address: String,
}

#[derive(Deserialize)]
struct EditServerForm {
    name: String,
    ip_address: String,
}

#[get("/servers/{id}/edit")]
pub async fn edit_server_page(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    path: web::Path<String>,
) -> impl Responder {
    let server_id = path.into_inner();
    match sqlx::query_as!(
        Server,
        r#"SELECT id as "id!", name as "name!", ip_address as "ip_address!", created_at as "created_at!" FROM servers WHERE id = ?"#,
        server_id
    )
    .fetch_optional(pool.as_ref())
    .await
    {
        Ok(Some(server)) => {
            let mut ctx = Context::new();
            ctx.insert("server", &server);
            match tera.render("edit_server.html", &ctx) {
                Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
                Err(e) => HttpResponse::InternalServerError().body(format!("Template error: {}", e)),
            }
        }
        Ok(None) => HttpResponse::NotFound().body("Server not found"),
        Err(_) => HttpResponse::InternalServerError().body("Database error"),
    }
}

#[post("/servers/{id}/edit")]
pub async fn edit_server(
    pool: web::Data<SqlitePool>,
    path: web::Path<String>,
    form: web::Form<EditServerForm>,
) -> impl Responder {
    let server_id = path.into_inner();
    let name = form.name.trim().to_string();
    let ip_address = form.ip_address.trim().to_string();
    if name.is_empty() || ip_address.is_empty() {
        return HttpResponse::Found()
            .append_header(("Location", format!("/servers/{}/edit", server_id)))
            .append_header(("Set-Cookie", "flash_message=Name and IP are required; Path=/; Max-Age=5"))
            .finish();
    }
    match sqlx::query!(
        r#"UPDATE servers SET name = ?, ip_address = ? WHERE id = ?"#,
        name,
        ip_address,
        server_id
    )
    .execute(pool.as_ref())
    .await
    {
        Ok(result) => {
            if result.rows_affected() > 0 {
                HttpResponse::Found()
                    .append_header(("Location", "/"))
                    .append_header(("Set-Cookie", "flash_message=Server updated successfully; Path=/; Max-Age=5"))
                    .finish()
            } else {
                HttpResponse::Found()
                    .append_header(("Location", "/"))
                    .append_header(("Set-Cookie", "flash_message=Server not found; Path=/; Max-Age=5"))
                    .finish()
            }
        }
        Err(e) => {
            let msg = if e.to_string().contains("UNIQUE constraint failed") {
                "Server with this IP address already exists"
            } else {
                "Failed to update server"
            };
            HttpResponse::Found()
                .append_header(("Location", format!("/servers/{}/edit", server_id)))
                .append_header(("Set-Cookie", format!("flash_message={}; Path=/; Max-Age=5", msg)))
                .finish()
        }
    }
}

#[post("/servers")]
pub async fn add_server(
    pool: web::Data<SqlitePool>,
    form: web::Form<NewServerForm>,
) -> impl Responder {
    let server_id = Uuid::new_v4().to_string();
    
    let now = Utc::now().naive_utc();
    
    match sqlx::query!(
        r#"
        INSERT INTO servers (id, name, ip_address, created_at)
        VALUES (?, ?, ?, ?)
        "#,
        server_id,
        form.name,
        form.ip_address,
        now
    )
    .execute(pool.as_ref())
    .await
    {
        Ok(_) => {
            HttpResponse::Found()
                .append_header(("Location", "/"))
                .append_header(("Set-Cookie", format!("flash_message=Server added successfully; Path=/; Max-Age=5")))
                .finish()
        }
        Err(e) => {
            error!("Database error: {}", e);
            let e: sqlx::Error = e;
            if e.to_string().contains("UNIQUE constraint failed") {
                HttpResponse::Found()
                    .append_header(("Location", "/"))
                    .append_header(("Set-Cookie", format!("flash_message=Server with this IP address already exists; Path=/; Max-Age=5")))
                    .finish()
            } else {
                HttpResponse::Found()
                    .append_header(("Location", "/"))
                    .append_header(("Set-Cookie", format!("flash_message=Failed to add server; Path=/; Max-Age=5")))
                    .finish()
            }
        }
    }
}

#[delete("/servers/{id}")]
pub async fn remove_server(
    pool: web::Data<SqlitePool>,
    path: web::Path<String>,
) -> impl Responder {
    let server_id = path.into_inner();
    
    match sqlx::query!("DELETE FROM servers WHERE id = ?", server_id)
        .execute(pool.as_ref())
        .await
    {
        Ok(result) => {
            if result.rows_affected() > 0 {
                HttpResponse::Found()
                    .append_header(("Location", "/"))
                    .append_header(("Set-Cookie", format!("flash_message=Server removed successfully; Path=/; Max-Age=5")))
                    .finish()
            } else {
                HttpResponse::Found()
                    .append_header(("Location", "/"))
                    .append_header(("Set-Cookie", format!("flash_message=Server not found; Path=/; Max-Age=5")))
                    .finish()
            }
        }
        Err(e) => {
            error!("Database error: {}", e);
            HttpResponse::Found()
                .append_header(("Location", "/"))
                .append_header(("Set-Cookie", format!("flash_message=Failed to remove server; Path=/; Max-Age=5")))
                .finish()
        }
    }
}

#[get("/history")]
pub async fn all_servers_history_page(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    query: Query<DashboardQuery>,
) -> impl Responder {
    let page = query.page.unwrap_or(1);
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = (page - 1) * limit;
    match get_servers_with_latest_stats_paginated(&pool, limit, offset, &query.search).await {
        Ok((servers, total_count)) => {
            let total_pages = ((total_count as u32 + limit - 1) / limit).max(1);
            let mut ctx = Context::new();
            ctx.insert("servers", &servers);
            ctx.insert("current_page", &page);
            ctx.insert("total_pages", &total_pages);
            ctx.insert("total_servers", &total_count);
            ctx.insert("limit", &limit);
            ctx.insert("search", &query.search);
            match tera.render("all_history.html", &ctx) {
                Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
                Err(e) => HttpResponse::InternalServerError().body(format!("Template error: {}", e)),
            }
        }
        Err(_) => HttpResponse::InternalServerError().body("Database error"),
    }
}

#[get("/history/{server_id}")]
pub async fn server_history_page(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    path: web::Path<String>,
    query: Query<ServerQuery>,
) -> impl Responder {
    let server_id = path.into_inner();
    
    // Get server info
    match sqlx::query_as!(
        Server,
        r#"
        SELECT id as "id!", name as "name!", ip_address as "ip_address!", created_at as "created_at!"
        FROM servers 
        WHERE id = ?
        "#,
        server_id
    )
    .fetch_optional(pool.as_ref())
    .await
    {
        Ok(Some(server)) => {
            let hours = match query.period.as_deref() {
                Some("1h") => 1,
                Some("2h") => 2,
                Some("3h") => 3,
                Some("24h") => 24,
                Some("7d") => 24 * 7,
                _ => 24,
            };
            
            let page = query.page.unwrap_or(1);
            let limit = 50;
            let offset = (page - 1) * limit;
            
            // Get history data
            match get_server_history_paginated(&pool, &server_id, hours, limit, offset).await {
                Ok((history, total_count)) => {
                    // Get average stats
                    match get_average_stats(&pool, &server_id, hours, &query.period.as_deref().unwrap_or("24h")).await {
                        Ok(average_stats) => {
                            // Prepare chart data (include rates for network and disk I/O charts)
                            let mut chart_data = serde_json::Map::new();
                            let mut labels = Vec::new();
                            let mut cpu_data = Vec::new();
                            let mut memory_data = Vec::new();
                            let mut disk_data = Vec::new();
                            let mut load_data = Vec::new();
                            let mut network_in_data = Vec::new();
                            let mut network_out_data = Vec::new();
                            let mut network_in_rate_data = Vec::new();
                            let mut network_out_rate_data = Vec::new();
                            let mut disk_read_rate_data = Vec::new();
                            let mut disk_write_rate_data = Vec::new();

                            fn to_json_num(v: f64) -> serde_json::Value {
                                serde_json::Value::Number(serde_json::Number::from_f64(v).unwrap_or(serde_json::Number::from(0)))
                            }

                            for stat in &history {
                                labels.push(stat.created_at.format("%H:%M").to_string());
                                cpu_data.push(stat.cpu_usage);
                                memory_data.push(stat.memory_usage);
                                disk_data.push(stat.disk_usage);
                                load_data.push(stat.load_avg);
                                network_in_data.push(stat.network_in);
                                network_out_data.push(stat.network_out);
                                // Rates in KB/s for charts
                                network_in_rate_data.push(stat.network_in_rate / 1024.0);
                                network_out_rate_data.push(stat.network_out_rate / 1024.0);
                                disk_read_rate_data.push(stat.disk_read_rate / 1024.0);
                                disk_write_rate_data.push(stat.disk_write_rate / 1024.0);
                            }

                            chart_data.insert("labels".to_string(), serde_json::Value::Array(
                                labels.into_iter().map(serde_json::Value::String).collect()
                            ));
                            chart_data.insert("cpu".to_string(), serde_json::Value::Array(
                                cpu_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("memory".to_string(), serde_json::Value::Array(
                                memory_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("disk".to_string(), serde_json::Value::Array(
                                disk_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("load".to_string(), serde_json::Value::Array(
                                load_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("network_in".to_string(), serde_json::Value::Array(
                                network_in_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("network_out".to_string(), serde_json::Value::Array(
                                network_out_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("network_in_rate".to_string(), serde_json::Value::Array(
                                network_in_rate_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("network_out_rate".to_string(), serde_json::Value::Array(
                                network_out_rate_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("disk_read_rate".to_string(), serde_json::Value::Array(
                                disk_read_rate_data.into_iter().map(to_json_num).collect()
                            ));
                            chart_data.insert("disk_write_rate".to_string(), serde_json::Value::Array(
                                disk_write_rate_data.into_iter().map(to_json_num).collect()
                            ));
                            
                            let total_pages = ((total_count as u32 + limit - 1) / limit).max(1);
                            let period_param = query.period.as_deref().unwrap_or("24h");

                            let chart_data_json = serde_json::to_string(&chart_data).unwrap_or_else(|_| "{}".to_string());
                            let mut context = Context::new();
                            context.insert("server", &server);
                            context.insert("history", &history);
                            context.insert("average_stats", &average_stats);
                            context.insert("chart_data", &chart_data);
                            context.insert("chart_data_json", &chart_data_json);
                            context.insert("current_page", &page);
                            context.insert("total_pages", &total_pages);
                            context.insert("time_window", &period_param);
                            context.insert("period", &period_param);
                            // For template display (KB/s)
                            context.insert("avg_net_in_rate_kb", &(average_stats.network_in_rate / 1024.0));
                            context.insert("avg_net_out_rate_kb", &(average_stats.network_out_rate / 1024.0));
                            
                            match tera.render("history.html", &context) {
                                Ok(rendered) => HttpResponse::Ok().content_type("text/html").body(rendered),
                                Err(e) => {
                                    error!(err = %e, "History template render failed");
                                    HttpResponse::InternalServerError().body("An error occurred. Please try again.")
                                }
                            }
                        }
                        Err(e) => {
                            error!("Error getting average stats: {}", e);
                            HttpResponse::InternalServerError().body("Failed to fetch average stats")
                        }
                    }
                }
                Err(e) => {
                    error!("Error getting history: {}", e);
                    HttpResponse::InternalServerError().body("Failed to fetch server history")
                }
            }
        }
        Ok(None) => {
            HttpResponse::NotFound().body("Server not found")
        }
        Err(e) => {
            error!("Database error: {}", e);
            HttpResponse::InternalServerError().body("Database error")
        }
    }
}

#[get("/history/{server_id}/export")]
pub async fn server_history_export(
    pool: web::Data<SqlitePool>,
    path: web::Path<String>,
    query: Query<ServerQuery>,
) -> impl Responder {
    let server_id = path.into_inner();
    let hours = match query.period.as_deref() {
        Some("1h") => 1,
        Some("2h") => 2,
        Some("3h") => 3,
        Some("24h") => 24,
        Some("7d") => 24 * 7,
        _ => 24,
    };

    let server = match sqlx::query_as!(
        Server,
        r#"SELECT id as "id!", name as "name!", ip_address as "ip_address!", created_at as "created_at!" FROM servers WHERE id = ?"#,
        server_id
    )
    .fetch_optional(pool.as_ref())
    .await
    {
        Ok(Some(s)) => s,
        Ok(None) => return HttpResponse::NotFound().body("Server not found"),
        Err(_) => return HttpResponse::InternalServerError().body("Database error"),
    };

    let (history, _) = match get_server_history_paginated(&pool, &server_id, hours, 10_000, 0).await {
        Ok(h) => h,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to fetch history"),
    };

    let mut csv = String::from("Timestamp,CPU %,Memory %,Disk %,Load Avg,Net In (KB/s),Net Out (KB/s),Disk Read (KB/s),Disk Write (KB/s),Users,Uptime (hrs)\n");
    for s in &history {
        let line = format!(
            "{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{},{:.1}\n",
            s.created_at,
            s.cpu_usage,
            s.memory_usage,
            s.disk_usage,
            s.load_avg,
            s.network_in_rate / 1024.0,
            s.network_out_rate / 1024.0,
            s.disk_read_rate / 1024.0,
            s.disk_write_rate / 1024.0,
            s.logged_users,
            s.uptime as f64 / 3600.0
        );
        csv.push_str(&line);
    }

    let filename = format!(
        "attachment; filename=\"history_{}_{}.csv\"",
        server.name.replace(' ', "_"),
        query.period.as_deref().unwrap_or("24h")
    );
    HttpResponse::Ok()
        .append_header(("Content-Type", "text/csv; charset=utf-8"))
        .append_header(("Content-Disposition", filename))
        .body(csv)
}

#[get("/export")]
pub async fn dashboard_export(pool: web::Data<SqlitePool>) -> impl Responder {
    let (servers, _) = match get_servers_with_latest_stats_paginated(pool.as_ref(), 10_000, 0, &None).await {
        Ok(s) => s,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to fetch data"),
    };
    let mut csv = String::from("Server Name,IP Address,CPU %,Memory %,Disk %,Load,Download (KB/s),Upload (KB/s),Uptime (hrs),Last Seen\n");
    for s in &servers {
        let (cpu, mem, disk, load, net_in, net_out, uptime, last_seen) = if let Some(st) = &s.latest_stats {
            (
                format!("{:.1}", st.cpu_usage),
                format!("{:.1}", st.memory_usage),
                format!("{:.1}", st.disk_usage),
                format!("{:.2}", st.load_avg),
                format!("{:.1}", st.network_in_rate / 1024.0),
                format!("{:.1}", st.network_out_rate / 1024.0),
                format!("{:.1}", st.uptime as f64 / 3600.0),
                st.created_at.to_string(),
            )
        } else {
            ("—".into(), "—".into(), "—".into(), "—".into(), "—".into(), "—".into(), "—".into(), "—".into())
        };
        let name_escaped = s.server.name.replace('"', "\"\"");
        let line = format!(
            "\"{}\",\"{}\",{},{},{},{},{},{},{},\"{}\"\n",
            name_escaped,
            s.server.ip_address,
            cpu,
            mem,
            disk,
            load,
            net_in,
            net_out,
            uptime,
            last_seen
        );
        csv.push_str(&line);
    }
    HttpResponse::Ok()
        .append_header(("Content-Type", "text/csv; charset=utf-8"))
        .append_header(("Content-Disposition", "attachment; filename=\"servers_export.csv\""))
        .body(csv)
}

#[get("/api/history/{server_id}")]
pub async fn server_history_api(
    pool: web::Data<SqlitePool>,
    path: web::Path<String>,
    query: Query<ServerQuery>,
) -> impl Responder {
    let server_id = path.into_inner();
    let hours = match query.period.as_deref() {
        Some("1h") => 1,
        Some("24h") => 24,
        Some("7d") => 24 * 7,
        _ => 24, // Default to 24 hours
    };
    
    let limit = query.limit.unwrap_or(100).min(1000); // Max 1000 points
    let offset = query.page.unwrap_or(1).saturating_sub(1) * limit;
    
    match get_server_history_paginated(&pool, &server_id, hours, limit, offset).await {
        Ok((history, _total_count)) => HttpResponse::Ok().json(history),
        Err(e) => {
            error!("Database error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch server history"
            }))
        }
    }
}

#[get("/api/average/{server_id}")]
pub async fn api_average(
    pool: web::Data<SqlitePool>,
    path: web::Path<String>,
    query: Query<ServerQuery>,
) -> impl Responder {
    let server_id = path.into_inner();
    let period = query.period.as_deref().unwrap_or("24h");
    
    let hours = match period {
        "1h" => 1,
        "24h" => 24,
        "7d" => 24 * 7,
        _ => 24,
    };
    
    match get_average_stats(&pool, &server_id, hours, period).await {
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e) => {
            error!("Database error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch average stats"
            }))
        }
    }
}

async fn get_all_servers(pool: &SqlitePool) -> Result<Vec<Server>, sqlx::Error> {
    let servers = sqlx::query_as!(
        Server,
        r#"
        SELECT id as "id!", name as "name!", ip_address as "ip_address!", created_at as "created_at!"
        FROM servers
        ORDER BY created_at DESC
        "#
    )
    .fetch_all(pool)
    .await?;
    
    Ok(servers)
}

async fn get_servers_with_latest_stats_paginated(
    pool: &SqlitePool,
    limit: u32,
    offset: u32,
    search: &Option<String>,
) -> Result<(Vec<ServerWithStats>, i64), sqlx::Error> {
    let total_count = if let Some(search_term) = search {
        let search_pattern = format!("%{}%", search_term);
        sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) as "count!"
            FROM servers 
            WHERE name LIKE ? OR ip_address LIKE ?
            "#,
            search_pattern,
            search_pattern
        )
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) as "count!"
            FROM servers
            "#
        )
        .fetch_one(pool)
        .await?
    };

    let servers = if let Some(search_term) = search {
        let search_pattern = format!("%{}%", search_term);
        sqlx::query_as!(
            Server,
            r#"
            SELECT id as "id!", name as "name!", ip_address as "ip_address!", created_at as "created_at!"
            FROM servers 
            WHERE name LIKE ? OR ip_address LIKE ?
            ORDER BY name ASC
            LIMIT ? OFFSET ?
            "#,
            search_pattern,
            search_pattern,
            limit,
            offset
        )
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as!(
            Server,
            r#"
            SELECT id as "id!", name as "name!", ip_address as "ip_address!", created_at as "created_at!"
            FROM servers
            ORDER BY name ASC
            LIMIT ? OFFSET ?
            "#,
            limit,
            offset
        )
        .fetch_all(pool)
        .await?
    };

    let mut result = Vec::new();
    
    for server in servers {
        let latest_stats = sqlx::query_as!(
            ServerStats,
            r#"
                 SELECT id as "id!", server_id as "server_id!", cpu_usage as "cpu_usage!", 
                     memory_usage as "memory_usage!", memory_total as "memory_total!",
                     disk_usage as "disk_usage!", load_avg as "load_avg!", 
                     logged_users as "logged_users!", network_in as "network_in!", 
                     network_out as "network_out!", network_in_rate as "network_in_rate!", network_out_rate as "network_out_rate!",
                     disk_read_rate as "disk_read_rate!", disk_write_rate as "disk_write_rate!", uptime as "uptime!", 
                     created_at as "created_at!"
            FROM server_stats
            WHERE server_id = ?
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            server.id
        )
        .fetch_optional(pool)
        .await?;
        
        result.push(ServerWithStats {
            server,
            latest_stats,
        });
    }
    
    Ok((result, total_count as i64))
}

async fn get_servers_with_latest_stats(pool: &SqlitePool) -> Result<Vec<ServerWithStats>, sqlx::Error> {
    let servers = get_all_servers(pool).await?;
    let mut result = Vec::new();
    
    for server in servers {
        let latest_stats = sqlx::query_as!(
            ServerStats,
            r#"
                 SELECT id as "id!", server_id as "server_id!", cpu_usage as "cpu_usage!", 
                     memory_usage as "memory_usage!", memory_total as "memory_total!",
                     disk_usage as "disk_usage!", load_avg as "load_avg!", 
                     logged_users as "logged_users!", network_in as "network_in!", 
                     network_out as "network_out!", network_in_rate as "network_in_rate!", network_out_rate as "network_out_rate!",
                     disk_read_rate as "disk_read_rate!", disk_write_rate as "disk_write_rate!", uptime as "uptime!", 
                     created_at as "created_at!"
            FROM server_stats
            WHERE server_id = ?
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            server.id
        )
        .fetch_optional(pool)
        .await?;
        
        result.push(ServerWithStats {
            server,
            latest_stats,
        });
    }
    
    Ok(result)
}

async fn get_server_history_paginated(
    pool: &SqlitePool,
    server_id: &str,
    hours: i64,
    limit: u32,
    offset: u32,
) -> Result<(Vec<ServerStats>, i64), sqlx::Error> {
    let since = (Utc::now() - Duration::hours(hours)).naive_utc();
    
    // Get total count first
    let total_count: i64 = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) as "count!: i32"
        FROM server_stats
        WHERE server_id = ? AND created_at >= ?
        "#,
        server_id,
        since
    )
    .fetch_one(pool)
    .await?
    .into(); // Convert i32 to i64
    
    // Get the paginated data
    let stats = sqlx::query_as!(
        ServerStats,
        r#"
        SELECT id as "id!", server_id as "server_id!", cpu_usage as "cpu_usage!", 
               memory_usage as "memory_usage!", memory_total as "memory_total!",
               disk_usage as "disk_usage!", load_avg as "load_avg!", 
               logged_users as "logged_users!", network_in as "network_in!", 
               network_out as "network_out!", network_in_rate as "network_in_rate!", network_out_rate as "network_out_rate!",
               disk_read_rate as "disk_read_rate!", disk_write_rate as "disk_write_rate!", uptime as "uptime!", 
               created_at as "created_at!"
        FROM server_stats
        WHERE server_id = ? AND created_at >= ?
        ORDER BY created_at ASC
        LIMIT ? OFFSET ?
        "#,
        server_id,
        since,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?;
    
    Ok((stats, total_count))
}

async fn get_server_history(
    pool: &SqlitePool,
    server_id: &str,
    hours: i64,
) -> Result<Vec<ServerStats>, sqlx::Error> {
    let since = (Utc::now() - Duration::hours(hours)).naive_utc();
    
    let stats = sqlx::query_as!(
        ServerStats,
        r#"
         SELECT id as "id!", server_id as "server_id!", cpu_usage as "cpu_usage!", 
             memory_usage as "memory_usage!", memory_total as "memory_total!",
             disk_usage as "disk_usage!", load_avg as "load_avg!", 
             logged_users as "logged_users!", network_in as "network_in!", 
             network_out as "network_out!", network_in_rate as "network_in_rate!", network_out_rate as "network_out_rate!",
             disk_read_rate as "disk_read_rate!", disk_write_rate as "disk_write_rate!", uptime as "uptime!", 
             created_at as "created_at!"
        FROM server_stats
        WHERE server_id = ? AND created_at >= ?
        ORDER BY created_at ASC
        "#,
        server_id,
        since
    )
    .fetch_all(pool)
    .await?;
    
    Ok(stats)
}

async fn get_average_stats(
    pool: &SqlitePool,
    server_id: &str,
    hours: i64,
    period: &str,
) -> Result<AverageStats, sqlx::Error> {
    let since = (Utc::now() - Duration::hours(hours)).naive_utc();

    let result = sqlx::query!(
        r#"
        SELECT
            AVG(cpu_usage) as cpu_usage,
            AVG(memory_usage) as memory_usage,
            AVG(disk_usage) as disk_usage,
            AVG(load_avg) as load_avg,
            AVG(network_in) as network_in,
            AVG(network_out) as network_out,
            AVG(network_in_rate) as network_in_rate,
            AVG(network_out_rate) as network_out_rate
        FROM server_stats
        WHERE server_id = ? AND created_at >= ?
        "#,
        server_id,
        since
    )
    .fetch_one(pool)
    .await?;

    Ok(AverageStats {
        cpu_usage: result.cpu_usage.unwrap_or(0.0),
        memory_usage: result.memory_usage.unwrap_or(0.0),
        disk_usage: result.disk_usage.unwrap_or(0.0),
        load_avg: result.load_avg.unwrap_or(0.0),
        network_in: result.network_in.unwrap_or(0.0),
        network_out: result.network_out.unwrap_or(0.0),
        network_in_rate: result.network_in_rate.unwrap_or(0.0),
        network_out_rate: result.network_out_rate.unwrap_or(0.0),
        period: period.to_string(),
    })
}

#[get("/login")]
pub async fn login_page(
    tera: web::Data<Tera>,
) -> impl Responder {
    let mut context = Context::new();
    
    HttpResponse::Ok()
        .content_type("text/html")
        .body(tera.render("login.html", &context).unwrap_or_else(|_| {
            "Template error".to_string()
        }))
}

#[post("/login")]
pub async fn login(
    credentials: web::Form<LoginCredentials>,
) -> impl Responder {
    // Authentication with specified credentials
    if credentials.username == "servers" && credentials.password == "Soft@26*" {
        let flash_message = FlashMessage {
            message: "Login successful!".to_string(),
            type_: "success".to_string(),
        };
        
        return HttpResponse::Found()
            .append_header(("Location", "/"))
            .append_header(("Set-Cookie", format!("flash_message={}:success; Path=/; HttpOnly", flash_message.message)))
            .append_header(("Set-Cookie", "authenticated=true; Path=/; HttpOnly"))
            .finish();
    }
    
    let flash_message = FlashMessage {
        message: "Invalid username or password".to_string(),
        type_: "danger".to_string(),
    };
    
    HttpResponse::Found()
        .append_header(("Location", "/login"))
        .append_header(("Set-Cookie", format!("flash_message={}:danger; Path=/; HttpOnly", flash_message.message)))
        .finish()
}

#[get("/logout")]
pub async fn logout() -> impl Responder {
    HttpResponse::Found()
        .append_header(("Location", "/login"))
        .append_header(("Set-Cookie", "authenticated=; Max-Age=0; Path=/"))
        .append_header(("Set-Cookie", "flash_message=; Max-Age=0; Path=/"))
        .finish()
}
