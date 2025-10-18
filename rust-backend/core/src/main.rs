use actix_web::{web, App, HttpServer, HttpResponse, Result, middleware};
use actix_cors::Cors;
use sqlx::postgres::PgPoolOptions;
use redis::Client as RedisClient;
use async_nats::Client as NatsClient;
use tracing::{info, error};
use std::sync::Arc;

mod config;
mod error;
mod routes;
mod websocket;
mod auth;

use config::Config;
use error::AppError;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
    pub redis: redis::Client,
    pub nats: Arc<async_nats::Client>,
    pub config: Config,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    info!("🚀 Starting VIBE-CODE-KILLER Backend...");

    // Load configuration
    dotenv::dotenv().ok();
    let config = Config::from_env().expect("Failed to load configuration");

    // Initialize database connection pool
    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    info!("✅ Database connected");

    // Run migrations
    sqlx::migrate!("../database/migrations")
        .run(&db)
        .await
        .expect("Failed to run migrations");

    info!("✅ Migrations completed");

    // Initialize Redis
    let redis = RedisClient::open(config.redis_url.clone())
        .expect("Failed to create Redis client");

    info!("✅ Redis connected");

    // Initialize NATS
    let nats = async_nats::connect(&config.nats_url)
        .await
        .expect("Failed to connect to NATS");

    info!("✅ NATS connected");

    let state = AppState {
        db,
        redis,
        nats: Arc::new(nats),
        config: config.clone(),
    };

    let server_addr = format!("{}:{}", config.host, config.port);
    info!("🌐 Starting server on {}", server_addr);

    HttpServer::new(move || {
        let cors = Cors::permissive();

        App::new()
            .app_data(web::Data::new(state.clone()))
            .wrap(cors)
            .wrap(middleware::Logger::default())
            .wrap(middleware::Compress::default())
            .service(
                web::scope("/api/v1")
                    .service(routes::auth::auth_routes())
                    .service(routes::projects::project_routes())
                    .service(routes::oversight::oversight_routes())
                    .service(routes::prompts::prompt_routes())
            )
            .route("/health", web::get().to(health_check))
            .route("/ws", web::get().to(websocket::ws_handler))
            .default_service(web::route().to(not_found))
    })
    .bind(server_addr)?
    .run()
    .await
}

async fn health_check(state: web::Data<AppState>) -> Result<HttpResponse> {
    // Check database
    let db_status = sqlx::query("SELECT 1")
        .fetch_one(&state.db)
        .await
        .is_ok();

    // Check Redis
    let redis_status = state.redis.get_connection().is_ok();

    if db_status && redis_status {
        Ok(HttpResponse::Ok().json(serde_json::json!({
            "status": "healthy",
            "database": "connected",
            "redis": "connected",
            "version": env!("CARGO_PKG_VERSION")
        })))
    } else {
        Ok(HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "status": "unhealthy",
            "database": if db_status { "connected" } else { "disconnected" },
            "redis": if redis_status { "connected" } else { "disconnected" }
        })))
    }
}

async fn not_found() -> Result<HttpResponse> {
    Ok(HttpResponse::NotFound().json(serde_json::json!({
        "error": "Route not found"
    })))
}