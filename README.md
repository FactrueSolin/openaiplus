# openaiplus

Minimal Rust API and static page for generating a ChatGPT Plus hosted Stripe checkout URL from a manually submitted ChatGPT session access token.

## Run

Default mode is reproducible and does not enable any proxy.

```powershell
Copy-Item config.example.toml config.toml
rtk cargo run
```

Open `http://127.0.0.1:3000`.

Optional proxy pool:

```powershell
$env:PROXY_POOL = "http://127.0.0.1:7890,socks5h://127.0.0.1:1080"
rtk cargo run
```

Each `POST /api/checkout` request starts from a random configured proxy. If connecting or sending through that proxy fails, the server tries the remaining proxies once in order and then returns the last send error. ChatGPT HTTP errors and invalid response bodies are returned directly without switching proxies. If `PROXY_POOL` is empty, the ChatGPT checkout request uses direct connection.

## API

`POST /api/checkout`

Request body accepts either `session_token` or `accessToken`:

```json
{
  "session_token": "CHATGPT_ACCESS_TOKEN"
}
```

The server calls `https://chatgpt.com/backend-api/payments/checkout` with the ChatGPT Plus hosted checkout payload and returns the hosted Stripe URL.

## Verify

```powershell
rtk cargo fmt --check
rtk cargo test
rtk cargo check
$env:PORT = "3401"; $env:PROXY_POOL = ""; rtk cargo run
```

In another terminal:

```powershell
Invoke-RestMethod http://127.0.0.1:3401/healthz
Invoke-WebRequest http://127.0.0.1:3401/
```
