use crate::models::Server;
use crate::services::MetricsService;
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::time::interval;
use tracing::{error, info};
use futures::stream::{FuturesUnordered, StreamExt};

pub async fn start_scheduler(pool: SqlitePool, collection_interval: u64) {
    info!(
        "Starting metrics collection scheduler with {} second interval",
        collection_interval
    );

    let metrics_service = MetricsService::new(pool.clone());
    let mut interval_timer = interval(Duration::from_secs(collection_interval));

    // Run once immediately
    collect_all_metrics(&metrics_service, &pool).await;

    loop {
        interval_timer.tick().await;
        collect_all_metrics(&metrics_service, &pool).await;
    }
}

async fn collect_all_metrics(metrics_service: &MetricsService, pool: &SqlitePool) {
    info!("Starting metrics collection cycle");

    match get_all_servers(pool).await {
        Ok(servers) if servers.is_empty() => {
            info!("No servers configured for monitoring");
        }

        Ok(servers) => {
            let server_count = servers.len();
            info!("Collecting metrics for {} servers", server_count);
            
            // For large numbers of servers, process in batches to avoid overwhelming the system
            let batch_size = std::cmp::min(50, server_count.max(1));
            let mut success_count = 0;
            let mut error_count = 0;

            // Clone servers to avoid borrowing issues
            let servers_clone = servers.clone();

            for chunk in servers_clone.chunks(batch_size) {
                let mut tasks = FuturesUnordered::new();

                for server in chunk {
                    let service = metrics_service.clone();
                    let server_clone = server.clone();

                    tasks.push(tokio::spawn(async move {
                        info!(
                            "Collecting metrics for server: {} ({})",
                            server_clone.name, server_clone.ip_address
                        );

                        match service.collect_and_save(&server_clone.ip_address, &server_clone.id).await {
                            Ok(_) => {
                                info!("Successfully collected and saved metrics for {}", server_clone.ip_address);
                                Ok(())
                            }
                            Err(e) => {
                                error!(
                                    "Failed to collect metrics for {}: {}",
                                    server_clone.name, e
                                );
                                Err(e)
                            }
                        }
                    }));
                }

                while let Some(result) = tasks.next().await {
                    match result {
                        Ok(Ok(_)) => success_count += 1,
                        Ok(Err(_)) => error_count += 1,
                        Err(e) => {
                            error!("Task failed: {}", e);
                            error_count += 1;
                        }
                    }
                }
                
                // Small delay between batches to prevent overwhelming the network
                if chunk.len() == batch_size {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }

            info!(
                "Completed metrics collection cycle: {} successful, {} failed",
                success_count, error_count
            );
        }

        Err(e) => {
            error!("Failed to fetch servers: {}", e);
        }
    }
}

async fn get_all_servers(pool: &SqlitePool) -> Result<Vec<Server>, sqlx::Error> {
    sqlx::query_as::<_, Server>(
        r#"
        SELECT id, name, ip_address, created_at
        FROM servers
        ORDER BY created_at DESC
        "#
    )
    .fetch_all(pool)
    .await
}
