use std::{env, fs, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::Rng;
use reqwest::{Client, Proxy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::services::ServeDir;
use tracing::{error, info, warn};

const CHATGPT_CHECKOUT_URL: &str = "https://chatgpt.com/backend-api/payments/checkout";

#[derive(Clone)]
struct AppState {
    checkout: ChatGptCheckoutClient,
}

#[derive(Clone)]
struct ChatGptCheckoutClient {
    direct_client: Client,
    proxy_clients: Arc<Vec<ProxyClient>>,
}

#[derive(Clone)]
struct ProxyClient {
    url: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct CheckoutRequest {
    #[serde(alias = "accessToken")]
    session_token: String,
}

#[derive(Debug, Serialize)]
struct CheckoutResponse {
    checkout_url: String,
    checkout_session_id: Option<String>,
    processor_entity: Option<String>,
    used_proxy: Option<String>,
    raw: Value,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    proxy_count: usize,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = AppConfig::load()?;
    info!(
        listen_addr = %config.listen_addr,
        proxy_count = config.proxy_pool.len(),
        "loaded application config"
    );
    let state = AppState {
        checkout: ChatGptCheckoutClient::new(config.proxy_pool)?,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/checkout", post(checkout))
        .fallback_service(ServeDir::new("static"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;
    info!(listen_addr = %config.listen_addr, "server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        proxy_count: state.checkout.proxy_count(),
    })
}

async fn checkout(
    State(state): State<AppState>,
    Json(request): Json<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, AppError> {
    if request.session_token.trim().is_empty() {
        warn!("checkout rejected: missing session token");
        return Err(AppError {
            status: StatusCode::BAD_REQUEST,
            message: "session_token/accessToken is required".to_string(),
        });
    }

    state.checkout.create_checkout(request).await.map(Json)
}

impl ChatGptCheckoutClient {
    fn new(proxy_pool: Vec<String>) -> Result<Self> {
        let direct_client = build_client(None)?;
        let mut proxy_clients = Vec::with_capacity(proxy_pool.len());

        for proxy_url in proxy_pool {
            let proxy = Proxy::all(&proxy_url)
                .with_context(|| format!("invalid proxy url: {proxy_url}"))?;
            proxy_clients.push(ProxyClient {
                url: proxy_url,
                client: build_client(Some(proxy))?,
            });
        }

        Ok(Self {
            direct_client,
            proxy_clients: Arc::new(proxy_clients),
        })
    }

    fn proxy_count(&self) -> usize {
        self.proxy_clients.len()
    }

    async fn create_checkout(
        &self,
        request: CheckoutRequest,
    ) -> Result<CheckoutResponse, AppError> {
        let payload = chatgpt_checkout_payload();
        let session_token = request.session_token.trim();

        if self.proxy_clients.is_empty() {
            return self
                .send_checkout_request(&self.direct_client, None, session_token, &payload)
                .await
                .map_err(checkout_attempt_error_into_app_error);
        }

        let start_index = rand::rng().random_range(0..self.proxy_clients.len());
        let mut last_send_error = None;

        for index in proxy_attempt_indices(self.proxy_clients.len(), start_index) {
            let proxy_client = &self.proxy_clients[index];
            match self
                .send_checkout_request(
                    &proxy_client.client,
                    Some(proxy_client.url.clone()),
                    session_token,
                    &payload,
                )
                .await
            {
                Ok(response) => return Ok(response),
                Err(CheckoutAttemptError::Send(err)) => {
                    warn!(
                        proxy = %proxy_client.url,
                        error = %err,
                        "checkout proxy attempt failed"
                    );
                    last_send_error = Some(err);
                }
                Err(CheckoutAttemptError::Final(err)) => {
                    error!(
                        proxy = %proxy_client.url,
                        status = %err.status,
                        error = %err.message,
                        "checkout proxy attempt returned terminal error"
                    );
                    return Err(err);
                }
            }
        }

        let message = match last_send_error {
            Some(err) => format!(
                "ChatGPT checkout request failed after {} proxy attempts: {err}",
                self.proxy_clients.len()
            ),
            None => "ChatGPT checkout request failed: no proxy attempted".to_string(),
        };

        Err(AppError::bad_gateway(message))
    }

    async fn send_checkout_request(
        &self,
        client: &Client,
        used_proxy: Option<String>,
        session_token: &str,
        payload: &Value,
    ) -> Result<CheckoutResponse, CheckoutAttemptError> {
        let upstream = used_proxy.as_deref().unwrap_or("direct");
        let response = client
            .post(CHATGPT_CHECKOUT_URL)
            .bearer_auth(session_token)
            .json(payload)
            .send()
            .await
            .map_err(CheckoutAttemptError::Send)?;

        let status = response.status();
        let body = response.json::<Value>().await.map_err(|err| {
            error!(
                upstream,
                status = %status,
                error = %err,
                "failed to parse ChatGPT checkout response"
            );
            CheckoutAttemptError::Final(AppError::bad_gateway(format!(
                "invalid ChatGPT checkout response: {err}"
            )))
        })?;

        if !status.is_success() {
            warn!(
                upstream,
                status = %status,
                response = %body,
                "ChatGPT checkout returned non-success status"
            );
            return Err(CheckoutAttemptError::Final(AppError::bad_gateway(format!(
                "ChatGPT checkout returned HTTP {status}: {body}"
            ))));
        }

        let checkout_url = extract_checkout_url(&body).ok_or_else(|| {
            error!(
                upstream,
                response = %body,
                "ChatGPT checkout response missing checkout url"
            );
            CheckoutAttemptError::Final(AppError::bad_gateway(format!(
                "missing checkout url in response: {body}"
            )))
        })?;

        Ok(CheckoutResponse {
            checkout_url,
            checkout_session_id: body
                .get("checkout_session_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            processor_entity: body
                .get("processor_entity")
                .and_then(Value::as_str)
                .map(str::to_owned),
            used_proxy,
            raw: body,
        })
    }
}

enum CheckoutAttemptError {
    Send(reqwest::Error),
    Final(AppError),
}

fn checkout_attempt_error_into_app_error(err: CheckoutAttemptError) -> AppError {
    match err {
        CheckoutAttemptError::Send(err) => {
            AppError::bad_gateway(format!("ChatGPT checkout request failed: {err}"))
        }
        CheckoutAttemptError::Final(err) => err,
    }
}

fn proxy_attempt_indices(proxy_count: usize, start_index: usize) -> impl Iterator<Item = usize> {
    (0..proxy_count).map(move |offset| (start_index + offset) % proxy_count)
}

fn build_client(proxy: Option<Proxy>) -> Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("openaiplus/0.1");

    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy);
    }

    builder.build().context("failed to build HTTP client")
}

fn chatgpt_checkout_payload() -> Value {
    json!({
        "plan_name": "chatgptplusplan",
        "billing_details": {
            "country": "US",
            "currency": "USD"
        },
        "cancel_url": "https://chatgpt.com/#pricing",
        "promo_campaign": {
            "promo_campaign_id": "plus-1-month-free",
            "is_coupon_from_query_param": false
        },
        "checkout_ui_mode": "hosted"
    })
}

fn extract_checkout_url(body: &Value) -> Option<String> {
    ["url", "stripe_hosted_url", "checkout_url"]
        .iter()
        .find_map(|key| body.get(key).and_then(Value::as_str))
        .map(str::to_owned)
}

struct AppConfig {
    listen_addr: SocketAddr,
    proxy_pool: Vec<String>,
}

#[derive(Default, Deserialize)]
struct FileConfig {
    host: Option<String>,
    port: Option<u16>,
    proxy_pool: Option<Vec<String>>,
}

impl AppConfig {
    fn load() -> Result<Self> {
        let file_config = read_file_config()?;
        let host = env::var("HOST")
            .ok()
            .or(file_config.host)
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let port = env::var("PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .or(file_config.port)
            .unwrap_or(3000);
        let listen_addr = format!("{host}:{port}")
            .parse()
            .with_context(|| format!("invalid listen address {host}:{port}"))?;
        let proxy_pool = env::var("PROXY_POOL")
            .ok()
            .map(|value| split_proxy_pool(&value))
            .unwrap_or_else(|| file_config.proxy_pool.unwrap_or_default());

        Ok(Self {
            listen_addr,
            proxy_pool,
        })
    }
}

fn read_file_config() -> Result<FileConfig> {
    let path = env::var("OPENAIPLUS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.toml"));

    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("invalid config file {}", path.display()))
}

fn split_proxy_pool(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|proxy| !proxy.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_session_token_or_access_token() {
        let session_token: CheckoutRequest =
            serde_json::from_value(json!({ "session_token": "session-value" })).unwrap();
        let access_token: CheckoutRequest =
            serde_json::from_value(json!({ "accessToken": "access-value" })).unwrap();

        assert_eq!(session_token.session_token, "session-value");
        assert_eq!(access_token.session_token, "access-value");
    }

    #[test]
    fn payload_matches_chatgpt_checkout_contract() {
        let payload = chatgpt_checkout_payload();

        assert_eq!(payload["plan_name"], "chatgptplusplan");
        assert_eq!(payload["billing_details"]["country"], "US");
        assert_eq!(payload["billing_details"]["currency"], "USD");
        assert_eq!(payload["checkout_ui_mode"], "hosted");
    }

    #[test]
    fn extracts_known_checkout_url_fields() {
        assert_eq!(
            extract_checkout_url(&json!({ "url": "https://example.com/a" })).as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            extract_checkout_url(&json!({ "stripe_hosted_url": "https://example.com/b" }))
                .as_deref(),
            Some("https://example.com/b")
        );
        assert_eq!(
            extract_checkout_url(&json!({ "checkout_url": "https://example.com/c" })).as_deref(),
            Some("https://example.com/c")
        );
    }

    #[test]
    fn splits_proxy_pool_from_env_style_value() {
        assert_eq!(
            split_proxy_pool("http://127.0.0.1:7890, socks5h://127.0.0.1:1080, "),
            vec![
                "http://127.0.0.1:7890".to_string(),
                "socks5h://127.0.0.1:1080".to_string()
            ]
        );
    }

    #[test]
    fn proxy_attempt_indices_try_each_proxy_once_from_random_start() {
        assert_eq!(
            proxy_attempt_indices(4, 2).collect::<Vec<_>>(),
            vec![2, 3, 0, 1]
        );
        assert_eq!(
            proxy_attempt_indices(3, 0).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(proxy_attempt_indices(0, 0).collect::<Vec<_>>().is_empty());
    }
}
