use actix_web::{App, HttpServer, middleware::Logger};
use actix_web::web::Data;
use dotenv::dotenv;
use sqlx::SqlitePool;
use std::env;
use tera::Tera;
use tracing::info;

mod models;
mod routes;
mod services;
mod scheduler;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt::init();

    // Get database URL from env or fallback
    let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite::memory:".to_string());
    info!("Connecting to database: {}", database_url);

    // Connect to database
    let pool: SqlitePool = SqlitePool::connect(&database_url)
        .await
        .expect("Failed to connect to database");

    // Run migrations
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    // Read server config from environment
    let port: u16 = env::var("SERVER_PORT")
        .unwrap_or_else(|_| "5400".to_string())
        .parse()
        .expect("Invalid SERVER_PORT");

    let collection_interval: u64 = env::var("COLLECTION_INTERVAL")
        .unwrap_or_else(|_| "30".to_string())
        .parse()
        .expect("Invalid COLLECTION_INTERVAL");

    // Initialize Tera templates
    let mut tera = Tera::new("templates/**/*").expect("Failed to compile templates");

    // Fix for template errors: make sure Tera does not fail on missing keys
    tera.autoescape_on(vec!["html"]);

    // Start background metrics collection
    let pool_for_scheduler = pool.clone();
    tokio::spawn(async move {
        scheduler::start_scheduler(pool_for_scheduler, collection_interval).await;
    });

    info!("Starting server on port {}", port);

    // Start Actix server
    HttpServer::new(move || {
        App::new()
            .app_data(Data::new(pool.clone()))
            .app_data(Data::new(tera.clone()))
            .service(routes::dashboard)
            .service(routes::servers_list)
            .service(routes::add_server)
            .service(routes::remove_server)
            .service(routes::server_history_page)
            .service(routes::server_history_api)
            .service(routes::api_average)
            .wrap(Logger::default())
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
