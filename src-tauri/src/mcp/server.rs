use axum::{
    body::Body,
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::any_service,
    Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tauri::AppHandle;

use crate::agents::AgentRuntime;

use super::{handler::SmoothMcpHandler, McpAuthState, MCP_PORT};

pub(crate) async fn run(app: AppHandle, runtime: AgentRuntime, auth_state: McpAuthState) {
    let handler_app = app.clone();
    let service = StreamableHttpService::new(
        move || Ok(SmoothMcpHandler::new(handler_app.clone(), runtime.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true),
    );
    let router = Router::new()
        .route_service("/mcp", any_service(service))
        .layer(middleware::from_fn_with_state(auth_state, authorize));
    let address = format!("127.0.0.1:{MCP_PORT}");
    match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => {
            if let Err(error) = axum::serve(listener, router).await {
                eprintln!("Smooth MCP server stopped: {error}");
            }
        }
        Err(error) => eprintln!("Smooth MCP server could not bind to {address}: {error}"),
    }
}

async fn authorize(
    State(auth_state): State<McpAuthState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = auth_state
        .0
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();
    let expected = format!("Bearer {token}");
    let authorized = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    if !authorized {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}
