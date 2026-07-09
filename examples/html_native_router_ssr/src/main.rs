//! Native authorized SSR HTTP server for the HTML router example.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::Cell;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use clap::Parser;
    use html_native_router::{
        dashboard_view, ClientState, APP_STATE, HEIGHT, ROUTE_PATTERN, WIDTH,
    };
    use schnellui_render_html::{
        Authorization, HtmlRenderer, HtmlRouter, RouteMatch, SsrAuthorize, SsrRoute,
    };

    #[derive(Debug, Parser)]
    #[command(
        name = "html_native_router_ssr",
        about = "authorized native-HTML SSR router and development server"
    )]
    struct Cli {
        /// Start the SSR HTTP server instead of rendering one document.
        #[arg(long)]
        serve: bool,

        /// Address used by `--serve`.
        #[arg(long, default_value = "127.0.0.1:8080")]
        address: SocketAddr,

        /// Trunk output containing index.html and the CSR JS/WASM assets.
        #[arg(long)]
        client_dir: Option<PathBuf>,

        /// Route rendered when not using `--serve`.
        #[arg(long, default_value = "/users/42/dashboard")]
        route: String,

        /// Write one rendered document to this file; stdout is the default.
        #[arg(long, conflicts_with = "serve")]
        out: Option<PathBuf>,
    }

    /// This value survives chained SSR stages but never crosses hydration.
    struct ServerState {
        database_password: &'static str,
        client: ClientState,
    }

    struct RequestContext {
        signed_in_user: Option<u64>,
        database_password: &'static str,
    }

    struct DashboardSsrRoute;

    impl SsrAuthorize<RequestContext> for DashboardSsrRoute {
        fn authorize(&self, context: &RequestContext, route: &RouteMatch) -> Authorization {
            let requested_user = route.param("user_id").and_then(|id| id.parse().ok());
            if requested_user == context.signed_in_user {
                Authorization::allow()
            } else {
                Authorization::deny("This dashboard belongs to another user")
            }
        }
    }

    impl SsrRoute<RequestContext> for DashboardSsrRoute {
        fn render(
            &self,
            renderer: &HtmlRenderer,
            context: &RequestContext,
            route: &RouteMatch,
        ) -> Result<schnellui_render_html::HtmlDocument, schnellui_render_html::HydrationError>
        {
            let user_id = route.param("user_id").unwrap_or_default().to_string();

            renderer
                .ssr((context.database_password, user_id))
                // SSR inside outer SSR: derive the nested server view while a
                // sensitive value remains confined to server state.
                .then(|(database_password, user_id)| ServerState {
                    database_password,
                    client: ClientState {
                        user_name: format!("User {user_id}"),
                        initial_count: 7,
                        inner_ssr_message: format!(
                            "Private report prepared on the server for user {user_id}"
                        ),
                    },
                })
                // CSR inside SSR: serialize only the explicitly selected value.
                .hydrate(APP_STATE, |server| server.client.clone())
                .map(|chain| {
                    chain.render(|server| {
                        let _server_only = server.database_password;
                        let client = server.client.clone();
                        let count = Rc::new(Cell::new(client.initial_count));
                        dashboard_view(client, count, route)
                    })
                })
        }
    }

    fn server_router() -> HtmlRouter<RequestContext> {
        HtmlRouter::new(HtmlRenderer::new(WIDTH, HEIGHT)).route(ROUTE_PATTERN, DashboardSsrRoute)
    }

    fn request_context() -> RequestContext {
        RequestContext {
            signed_in_user: Some(42),
            database_password: "server-only-database-password",
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::parse();
        if cli.serve {
            serve(cli)
        } else {
            render_once(cli)
        }
    }

    fn render_once(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
        let context = request_context();
        let response = server_router().render(cli.route, &context)?;
        let html = response.into_document().into_string();
        assert!(!html.contains(context.database_password));

        if let Some(path) = cli.out {
            fs::write(&path, html)?;
            println!("wrote {}", path.display());
        } else {
            print!("{html}");
        }
        Ok(())
    }

    fn serve(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
        let client_dir = cli.client_dir.unwrap_or_else(default_client_dir);
        let client_head = load_client_head(&client_dir)?;
        let listener = TcpListener::bind(cli.address)?;
        let router = server_router();
        let context = request_context();

        eprintln!(
            "SSR server listening on http://{}/users/42/dashboard",
            cli.address
        );
        eprintln!("serving CSR assets from {}", client_dir.display());

        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    if let Err(error) =
                        handle_request(&mut stream, &router, &context, &client_dir, &client_head)
                    {
                        eprintln!("request failed: {error}");
                    }
                }
                Err(error) => eprintln!("connection failed: {error}"),
            }
        }
        Ok(())
    }

    fn default_client_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../html_native_router/dist")
    }

    fn load_client_head(client_dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let index_path = client_dir.join("index.html");
        let index = fs::read_to_string(&index_path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                "cannot read CSR bundle {}: {error}; run `cd examples/html_native_router && trunk build` first",
                index_path.display()
                ),
            )
        })?;
        extract_head(&index).map(str::to_owned).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} has no <head> element", index_path.display()),
            )
            .into()
        })
    }

    fn extract_head(document: &str) -> Option<&str> {
        let start = document.find("<head>")? + "<head>".len();
        let end = document[start..].find("</head>")? + start;
        Some(&document[start..end])
    }

    fn inject_client_head(mut document: String, client_head: &str) -> String {
        let position = document
            .find("</head>")
            .expect("HtmlRenderer always emits a head element");
        document.insert_str(position, client_head);
        document
    }

    fn handle_request(
        stream: &mut TcpStream,
        router: &HtmlRouter<RequestContext>,
        context: &RequestContext,
        client_dir: &Path,
        client_head: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut request_line = String::new();
        BufReader::new(&mut *stream).read_line(&mut request_line)?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let location = parts.next().unwrap_or("/");

        if method != "GET" {
            return write_response(
                stream,
                405,
                "text/plain; charset=utf-8",
                b"Method not allowed",
            );
        }
        if location == "/" {
            return write_redirect(stream, "/users/42/dashboard");
        }

        let asset_path = location.split('?').next().unwrap_or(location);
        if let Some(asset) = safe_asset_name(asset_path) {
            let path = client_dir.join(asset);
            if path.is_file() {
                let bytes = fs::read(&path)?;
                return write_response(stream, 200, content_type(&path), &bytes);
            }
        }

        let response = router.render(location, context)?;
        let status = response.status();
        let html = response.into_document().into_string();
        let html = if status == 200 {
            inject_client_head(html, client_head)
        } else {
            html
        };
        write_response(stream, status, "text/html; charset=utf-8", html.as_bytes())
    }

    fn safe_asset_name(path: &str) -> Option<&str> {
        let name = path.strip_prefix('/')?;
        (!name.is_empty() && !name.contains('/') && name != "." && name != "..").then_some(name)
    }

    fn content_type(path: &Path) -> &'static str {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("js") => "text/javascript; charset=utf-8",
            Some("wasm") => "application/wasm",
            Some("css") => "text/css; charset=utf-8",
            _ => "application/octet-stream",
        }
    }

    fn write_redirect(
        stream: &mut TcpStream,
        location: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
        Ok(())
    }

    fn write_response(
        stream: &mut TcpStream,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let reason = match status {
            200 => "OK",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            _ => "Response",
        };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        stream.write_all(body)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn authorized_ssr_contains_hydration_but_not_the_secret() {
            let context = request_context();
            let response = server_router()
                .render("/users/42/dashboard", &context)
                .unwrap();
            let html = response.document().as_str();

            assert_eq!(response.status(), 200);
            assert!(html.contains("Outer SSR router"));
            assert!(html.contains("Hydrated CSR for User 42"));
            assert!(html.contains("schnellui-hydration"));
            assert!(!html.contains(context.database_password));
        }

        #[test]
        fn unauthorized_ssr_never_emits_hydration() {
            let context = RequestContext {
                signed_in_user: Some(7),
                database_password: "test-server-secret",
            };
            let response = server_router()
                .render("/users/42/dashboard", &context)
                .unwrap();

            assert_eq!(response.status(), 403);
            assert!(!response.document().as_str().contains("schnellui-hydration"));
        }

        #[test]
        fn trunk_head_is_injected_into_the_ssr_document() {
            let head = extract_head("<html><head><script>start()</script></head></html>").unwrap();
            let html = inject_client_head("<html><head></head><body></body></html>".into(), head);
            assert!(html.contains("<head><script>start()</script></head>"));
        }

        #[test]
        fn asset_paths_cannot_escape_the_client_directory() {
            assert_eq!(safe_asset_name("/client.js"), Some("client.js"));
            assert_eq!(safe_asset_name("/../secret"), None);
            assert_eq!(safe_asset_name("/nested/client.js"), None);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    native::run()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
