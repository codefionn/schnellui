use std::{net::SocketAddr, str::FromStr};

use secure_todo_ssr::{app, AppConfig};

#[tokio::main]
async fn main() {
    let address = std::env::var("TODO_BIND")
        .ok()
        .and_then(|value| SocketAddr::from_str(&value).ok())
        .unwrap_or_else(|| "127.0.0.1:3000".parse().expect("valid fallback address"));
    let secure_cookies = std::env::var("TODO_SECURE_COOKIES")
        .map(|value| value != "0")
        .unwrap_or(true);

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("bind TODO_BIND address");
    eprintln!("Secure Todo SSR listening on http://{address} (secure cookies: {secure_cookies})");
    axum::serve(listener, app(AppConfig { secure_cookies }))
        .await
        .expect("serve secure todo");
}
