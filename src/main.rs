use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use clap::Parser;
use natpmp::{Natpmp, Protocol, Response};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "natpmp-server")]
#[command(about = "NAT-PMP HTTP Server for Kubernetes")]
struct Args {
    /// VPN gateway IP address
    #[arg(long, required = true, env = "NATPMP_GATEWAY")]
    gateway: IpAddr,

    /// Server bind address
    #[arg(long, default_value = "0.0.0.0", env = "NATPMP_BIND_ADDRESS")]
    bind_address: IpAddr,

    /// Server port
    #[arg(long, default_value = "8080", env = "NATPMP_PORT")]
    port: u16,

    /// Maximum mapping duration in seconds (-1 to disable limit)
    #[arg(long, default_value = "300", env = "NATPMP_MAX_DURATION")]
    max_duration: i32,

    /// Log level
    #[arg(long, default_value = "info", env = "NATPMP_LOG_LEVEL")]
    log_level: String,
}

#[derive(Clone)]
struct AppState {
    gateway: IpAddr,
    max_duration: Option<u32>,
    token: Option<String>,
}

#[derive(Deserialize)]
struct ForwardRequest {
    internal_port: u16,
    protocol: String,
    duration: u32,
}

#[derive(Serialize)]
struct ForwardResponse {
    internal_port: u16,
    external_port: u16,
    protocol: String,
    duration: u32,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    timestamp: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

fn check_authorization(headers: &HeaderMap, expected_token: &Option<String>) -> bool {
    match expected_token {
        None => true, // No token required
        Some(token) => {
            if let Some(auth_header) = headers.get("authorization") {
                if let Ok(auth_str) = auth_header.to_str() {
                    return auth_str == format!("Bearer {}", token);
                }
            }
            false
        }
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

async fn forward(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ForwardRequest>,
) -> Result<Json<ForwardResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Check authorization
    if !check_authorization(&headers, &state.token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Unauthorized".to_string(),
            }),
        ));
    }

    // Validate and clamp duration
    let duration = match state.max_duration {
        Some(max) => payload.duration.min(max),
        None => payload.duration, // No limit if max_duration is -1
    };

    // Convert IpAddr to Ipv4Addr
    let gateway_v4 = match state.gateway {
        IpAddr::V4(ipv4) => ipv4,
        IpAddr::V6(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "IPv6 gateways not supported".to_string(),
                }),
            ));
        }
    };

    // Create NAT-PMP client
    let mut client = match Natpmp::new_with(gateway_v4) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create NAT-PMP client: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to create NAT-PMP client".to_string(),
                }),
            ));
        }
    };

    // Request port mapping (validates protocol implicitly)
    let protocol_enum = match payload.protocol.to_lowercase().as_str() {
        "tcp" => Protocol::TCP,
        "udp" => Protocol::UDP,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "protocol must be tcp or udp".to_string(),
                }),
            ));
        }
    };

    // Send the request
    if let Err(e) = client.send_port_mapping_request(
        protocol_enum,
        payload.internal_port,
        0, // Let NAT-PMP choose external port
        duration,
    ) {
        error!("Failed to send port mapping request: {}", e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to send port mapping request".to_string(),
            }),
        ));
    }

    // Wait a bit for the response
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

    // Read the response
    match client.read_response_or_retry() {
        Ok(response) => {
            let external_port = match response {
                Response::UDP(ur) => ur.public_port(),
                Response::TCP(tr) => tr.public_port(),
                _ => {
                    error!("Unexpected response type");
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "Unexpected response type".to_string(),
                        }),
                    ));
                }
            };

            info!(
                "Created mapping: {}/{} -> {} (duration: {}s)",
                payload.internal_port,
                payload.protocol.to_lowercase(),
                external_port,
                duration
            );

            Ok(Json(ForwardResponse {
                internal_port: payload.internal_port,
                external_port,
                protocol: payload.protocol.to_lowercase(),
                duration,
            }))
        }
        Err(e) => {
            error!("Failed to read port mapping response: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to read port mapping response".to_string(),
                }),
            ))
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    if args.log_level == "debug" {
                        "debug".into()
                    } else {
                        format!("natpmp_server={},tower_http=info", args.log_level).into()
                    }
                }),
        )
        .init();

    let state = AppState {
        gateway: args.gateway,
        max_duration: if args.max_duration == -1 {
            None
        } else {
            Some(args.max_duration as u32)
        },
        token: std::env::var("NATPMP_TOKEN").ok(),
    };

    // Build our application with routes
    let app = Router::new()
        .route("/health", get(health))
        .route("/forward", post(forward))
        .layer(
        TraceLayer::new_for_http()
            .make_span_with(tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::INFO))
            .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::INFO))
            .on_response(tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO))
    )
        .with_state(state);

    let bind_addr = format!("{}:{}", args.bind_address, args.port);
    let listener = TcpListener::bind(&bind_addr).await.unwrap();

    let token_env = std::env::var("NATPMP_TOKEN").ok();
    if token_env.is_some() {
        info!(
            "Starting NAT-PMP server on {} with gateway {} (auth enabled)",
            bind_addr, args.gateway
        );
    } else {
        warn!(
            "Starting NAT-PMP server on {} with gateway {} (no auth - consider using NATPMP_TOKEN)",
            bind_addr, args.gateway
        );
    }

    // Setup graceful shutdown for multiple signals
    let shutdown_signal = async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            
            let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
            let mut sigterm = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
            
            tokio::select! {
                _ = sigint.recv() => info!("Received SIGINT, initiating graceful shutdown..."),
                _ = sigterm.recv() => info!("Received SIGTERM, initiating graceful shutdown..."),
            }
        }
        
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install signal handler");
            info!("Received shutdown signal, initiating graceful shutdown...");
        }
    };

    // Run server with graceful shutdown
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal);
    
    if let Err(e) = server.await {
        error!("Server error: {}", e);
    } else {
        info!("Server shutdown complete");
    }
}
