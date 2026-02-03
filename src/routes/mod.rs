use actix_web::{get, post, delete, web, HttpResponse, Responder};
use actix_web::web::Query;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use crate::models::{Server, ServerStats, ServerWithStats, NewServer, AverageStats};
use chrono::{Utc, Duration};
use tera::{Tera, Context};
use uuid::Uuid;
use std::error::Error;
use tracing::info;

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
    type_: String,
}

#[get("/")]
pub async fn dashboard(
    pool: web::Data<SqlitePool>,
    tera: web::Data<Tera>,
    query: Query<DashboardQuery>,
) -> impl Responder {
    let page = query.page.unwrap_or(1);
    let limit = query.limit.unwrap_or(50).min(100); // Max 100 per page
    let offset = (page - 1) * limit;
    
    match get_servers_with_latest_stats_paginated(&pool, limit, offset, &query.search).await {
        Ok((servers, total_count)) => {
            let total_pages = ((total_count as u32 + limit - 1) / limit) as u32;
            
            // Debug logging
            info!("Pagination debug - total_count: {}, limit: {}, total_pages: {}, current_page: {}", 
                  total_count, limit, total_pages, page);
            
            let mut context = Context::new();
            context.insert("servers", &servers);
            context.insert("current_page", &page);
            context.insert("total_pages", &total_pages);
            context.insert("total_servers", &total_count);
            context.insert("limit", &limit);
            context.insert("search", &query.search);
            
            match tera.render("dashboard.html", &context) {
                Ok(rendered) => HttpResponse::Ok().content_type("text/html").body(rendered),
                Err(e) => {
                    let error_details = format!("Template error: {}\n\nSource: {:?}\n\nContext: {:#?}", 
                        e, e.source(), &context);
                    eprintln!("{}", error_details);
                    HttpResponse::InternalServerError().body(error_details)
                }
            }
        }
        Err(e) => {
            eprintln!("Database error: {}", e);
            HttpResponse::InternalServerError().body("Database error")
        }
    }
}

#[get("/servers")]
pub async fn servers_list(pool: web::Data<SqlitePool>) -> impl Responder {
    match get_all_servers(&pool).await {
        Ok(servers) => HttpResponse::Ok().json(servers),
        Err(e) => {
            eprintln!("Database error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to fetch servers"
            }))
        }
    }
}

#[post("/servers")]
pub async fn add_server(
    pool: web::Data<SqlitePool>,
    new_server: web::Json<NewServer>,
) -> impl Responder {
    let server_id = Uuid::new_v4().to_string();
    
    let now = Utc::now().naive_utc();
    
    match sqlx::query!(
        r#"
        INSERT INTO servers (id, name, ip_address, created_at)
        VALUES (?, ?, ?, ?)
        "#,
        server_id,
        new_server.name,
        new_server.ip_address,
        now
    )
    .execute(pool.as_ref())
    .await
    {
        Ok(_) => {
            HttpResponse::Ok().json(serde_json::json!({
                "message": "Server added successfully",
                "server_id": server_id
            }))
        }
        Err(e) => {
            eprintln!("Database error: {}", e);
            let e: sqlx::Error = e;
            if e.to_string().contains("UNIQUE constraint failed") {
                HttpResponse::BadRequest().json(serde_json::json!({
                    "error": "Server with this IP address already exists"
                }))
            } else {
                HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": "Failed to add server"
                }))
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
                HttpResponse::Ok().json(serde_json::json!({
                    "message": "Server removed successfully"
                }))
            } else {
                HttpResponse::NotFound().json(serde_json::json!({
                    "error": "Server not found"
                }))
            }
        }
        Err(e) => {
            eprintln!("Database error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to remove server"
            }))
        }
    }
}

#[get("/history/{server_id}")]
pub async fn server_history(
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
        Ok(stats) => HttpResponse::Ok().json(stats),
        Err(e) => {
            eprintln!("Database error: {}", e);
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
            eprintln!("Database error: {}", e);
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
                   network_out as "network_out!", uptime as "uptime!", 
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
                   network_out as "network_out!", uptime as "uptime!", 
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
) -> Result<Vec<ServerStats>, sqlx::Error> {
    let since = (Utc::now() - Duration::hours(hours)).naive_utc();
    
    let stats = sqlx::query_as!(
        ServerStats,
        r#"
        SELECT id as "id!", server_id as "server_id!", cpu_usage as "cpu_usage!", 
               memory_usage as "memory_usage!", memory_total as "memory_total!",
               disk_usage as "disk_usage!", load_avg as "load_avg!", 
               logged_users as "logged_users!", network_in as "network_in!", 
               network_out as "network_out!", uptime as "uptime!", 
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
    
    Ok(stats)
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
               network_out as "network_out!", uptime as "uptime!", 
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
            AVG(network_out) as network_out
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
        period: period.to_string(),
    })
}
