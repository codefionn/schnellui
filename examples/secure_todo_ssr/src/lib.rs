//! A deliberately small, auditable SSR todo example.
//!
//! Authentication, sessions, CSRF validation, and ownership checks all live on
//! the server.  The `HtmlRouter` is used as the authorization gate for the two
//! protected GET views; axum only turns its result into an HTTP response.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use rand::{distributions::Alphanumeric, rngs::OsRng, Rng};
use schnellui_render_html::{
    Authorization, HtmlRenderer, HtmlRouter, RouteMatch, SsrAuthorize, SsrRoute,
};
use schnellui_template::Text;
use serde::Deserialize;

const HOST_SESSION_COOKIE: &str = "__Host-secure_todo";
const HOST_LOGIN_CSRF_COOKIE: &str = "__Host-secure_todo_login_csrf";
const LOCAL_SESSION_COOKIE: &str = "secure_todo_session";
const LOCAL_LOGIN_CSRF_COOKIE: &str = "secure_todo_login_csrf";
const MAX_TITLE_LEN: usize = 160;

/// Runtime settings.  Set `secure_cookies` to false only for local HTTP tests.
#[derive(Clone, Copy, Debug)]
pub struct AppConfig {
    pub secure_cookies: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            secure_cookies: true,
        }
    }
}

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<Store>>,
    config: AppConfig,
    ssr: Arc<HtmlRouter<SsrContext>>,
}

#[derive(Clone)]
struct SsrContext {
    state: AppState,
    session: Option<Session>,
}

#[derive(Clone)]
struct User {
    id: u64,
    username: &'static str,
    password_hash: String,
}

#[derive(Clone)]
struct Session {
    user_id: u64,
    csrf: String,
}

#[derive(Clone)]
struct Todo {
    id: u64,
    owner_id: u64,
    title: String,
    completed: bool,
}

struct Store {
    users: Vec<User>,
    dummy_password_hash: String,
    sessions: HashMap<String, Session>,
    login_csrf: HashSet<String>,
    todos: Vec<Todo>,
    next_todo_id: u64,
}

struct TodosRoute;
struct TodoDetailRoute;

impl SsrAuthorize<SsrContext> for TodosRoute {
    fn authorize(&self, context: &SsrContext, _: &RouteMatch) -> Authorization {
        context
            .session
            .as_ref()
            .map(|_| Authorization::allow())
            .unwrap_or_else(|| Authorization::deny_with_status(401, "Sign in required"))
    }
}

impl SsrRoute<SsrContext> for TodosRoute {
    fn render(
        &self,
        renderer: &HtmlRenderer,
        _: &SsrContext,
        _: &RouteMatch,
    ) -> Result<schnellui_render_html::HtmlDocument, schnellui_render_html::HydrationError> {
        // The router owns the mandatory authorization + render dispatch. The
        // axum layer supplies the page shell because this app is ordinary HTML
        // forms rather than a hydrated schnellui component tree.
        Ok(renderer.render(Text::new("Authorized todo list")))
    }
}

impl SsrAuthorize<SsrContext> for TodoDetailRoute {
    fn authorize(&self, context: &SsrContext, route: &RouteMatch) -> Authorization {
        let Some(user_id) = context.session.as_ref().map(|session| session.user_id) else {
            return Authorization::deny_with_status(401, "Sign in required");
        };
        let Some(todo_id) = route
            .param("id")
            .and_then(|value| value.parse::<u64>().ok())
        else {
            return Authorization::deny_with_status(404, "Todo not found");
        };
        let owned = context
            .state
            .store
            .lock()
            .expect("store mutex poisoned")
            .todos
            .iter()
            .any(|todo| todo.id == todo_id && todo.owner_id == user_id);
        if owned {
            Authorization::allow()
        } else {
            // Conceal another user's IDs as well as absent IDs.
            Authorization::deny_with_status(404, "Todo not found")
        }
    }
}

impl SsrRoute<SsrContext> for TodoDetailRoute {
    fn render(
        &self,
        renderer: &HtmlRenderer,
        _: &SsrContext,
        _: &RouteMatch,
    ) -> Result<schnellui_render_html::HtmlDocument, schnellui_render_html::HydrationError> {
        Ok(renderer.render(Text::new("Authorized todo detail")))
    }
}

/// Builds the application. State is in memory so every new app gets fresh demo data.
pub fn app(config: AppConfig) -> Router {
    let store = Store::seeded();
    let renderer = HtmlRenderer::new(1280, 800);
    let ssr = Arc::new(
        HtmlRouter::new(renderer)
            .route("/todos", TodosRoute)
            .route("/todos/:id", TodoDetailRoute),
    );
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        config,
        ssr,
    };

    Router::new()
        .route("/", get(home))
        .route("/login", get(login_page).post(login))
        .route("/todos", get(todos).post(add_todo))
        .route("/todos/{id}", get(todo_detail))
        .route("/todos/{id}/toggle", post(toggle_todo))
        .route("/todos/{id}/delete", post(delete_todo))
        .route("/logout", post(logout))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(add_security_headers))
        .with_state(state)
}

impl Store {
    fn seeded() -> Self {
        let alice_hash = demo_hash("demo-password", "alice-demo-salt!");
        let bob_hash = demo_hash("demo-password", "bob-demo-salt!!!");
        Self {
            users: vec![
                User {
                    id: 1,
                    username: "alice",
                    password_hash: alice_hash,
                },
                User {
                    id: 2,
                    username: "bob",
                    password_hash: bob_hash,
                },
            ],
            dummy_password_hash: demo_hash("not-the-demo-password", "dummy-demo-salt!"),
            sessions: HashMap::new(),
            login_csrf: HashSet::new(),
            todos: vec![
                Todo {
                    id: 1,
                    owner_id: 1,
                    title: "Read the field notes".into(),
                    completed: false,
                },
                Todo {
                    id: 2,
                    owner_id: 1,
                    title: "Send the Monday dispatch".into(),
                    completed: true,
                },
                Todo {
                    id: 3,
                    owner_id: 2,
                    title: "Calibrate the tiny telescope".into(),
                    completed: false,
                },
            ],
            next_todo_id: 4,
        }
    }
}

fn demo_hash(password: &str, salt: &str) -> String {
    let salt = SaltString::encode_b64(salt.as_bytes()).expect("fixed demo salt is valid");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 can hash demo password")
        .to_string()
}

async fn home(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if current_session(&state, &headers).is_some() {
        redirect("/todos")
    } else {
        redirect("/login")
    }
}

async fn login_page(State(state): State<AppState>) -> Response {
    let csrf = token();
    state
        .store
        .lock()
        .expect("store mutex poisoned")
        .login_csrf
        .insert(csrf.clone());
    let cookie = cookie(
        login_csrf_cookie_name(state.config),
        &csrf,
        state.config,
        true,
        false,
    );
    page_response(StatusCode::OK, login_html(&csrf, None), Some(cookie))
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    csrf: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    // Consume a valid token before validating credentials or field lengths: a
    // captured form submission cannot be replayed down a different failure path.
    let csrf_ok = take_login_csrf(&state, &headers, &form.csrf);
    if form.username.len() > 64 || form.password.len() > 256 || !csrf_ok {
        return login_failure(&state, "The sign-in form expired. Please try again.");
    }
    let (user, password_hash) = {
        let store = state.store.lock().expect("store mutex poisoned");
        let user = store
            .users
            .iter()
            .find(|user| user.username == form.username)
            .cloned();
        let hash = user
            .as_ref()
            .map(|user| user.password_hash.clone())
            .unwrap_or_else(|| store.dummy_password_hash.clone());
        (user, hash)
    };
    // Argon2 is intentionally costly; keep it off the async worker thread.
    let password = form.password;
    let password_ok =
        tokio::task::spawn_blocking(move || verify_password(&password_hash, &password))
            .await
            .unwrap_or(false);
    if user.is_none() || !password_ok {
        return login_failure(&state, "That username and password do not match.");
    }
    let user = user.expect("checked above");
    let session_id = token();
    let session = Session {
        user_id: user.id,
        csrf: token(),
    };
    let mut store = state.store.lock().expect("store mutex poisoned");
    if let Some(previous_session) = session_cookie(&headers, state.config) {
        store.sessions.remove(&previous_session);
    }
    store.sessions.insert(session_id.clone(), session);
    drop(store);
    let mut response = redirect("/todos");
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie(
            session_cookie_name(state.config),
            &session_id,
            state.config,
            true,
            false,
        ),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie(
            login_csrf_cookie_name(state.config),
            "",
            state.config,
            true,
            true,
        ),
    );
    response
}

fn login_failure(state: &AppState, message: &str) -> Response {
    let csrf = token();
    state
        .store
        .lock()
        .expect("store mutex poisoned")
        .login_csrf
        .insert(csrf.clone());
    page_response(
        StatusCode::UNAUTHORIZED,
        login_html(&csrf, Some(message)),
        Some(cookie(
            login_csrf_cookie_name(state.config),
            &csrf,
            state.config,
            true,
            false,
        )),
    )
}

async fn todos(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return redirect("/login");
    };
    let context = SsrContext {
        state: state.clone(),
        session: Some(session.clone()),
    };
    match state.ssr.render("/todos", &context) {
        Ok(route) if route.status() == 200 => {
            let store = state.store.lock().expect("store mutex poisoned");
            let user = user_name(&store, session.user_id).unwrap_or("member");
            let todos = store
                .todos
                .iter()
                .filter(|todo| todo.owner_id == session.user_id)
                .collect::<Vec<_>>();
            page_response(
                StatusCode::OK,
                todos_html(user, &session.csrf, &todos),
                None,
            )
        }
        Ok(route) => error_response(route.status(), "Access denied"),
        Err(_) => error_response(500, "Unable to render this page"),
    }
}

async fn todo_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Response {
    let Some(session) = current_session(&state, &headers) else {
        return redirect("/login");
    };
    let context = SsrContext {
        state: state.clone(),
        session: Some(session.clone()),
    };
    match state.ssr.render(format!("/todos/{id}"), &context) {
        Ok(route) if route.status() == 200 => {
            let store = state.store.lock().expect("store mutex poisoned");
            let todo = store
                .todos
                .iter()
                .find(|todo| todo.id == id && todo.owner_id == session.user_id)
                .expect("router authorized owned todo");
            page_response(StatusCode::OK, detail_html(todo, &session.csrf), None)
        }
        Ok(route) => error_response(route.status(), "Todo not found"),
        Err(_) => error_response(500, "Unable to render this page"),
    }
}

#[derive(Deserialize)]
struct TodoForm {
    title: String,
    csrf: String,
}
#[derive(Deserialize)]
struct CsrfForm {
    csrf: String,
}

async fn add_todo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TodoForm>,
) -> Response {
    let Some(session) = csrf_session(&state, &headers, &form.csrf) else {
        return error_response(403, "Invalid request token");
    };
    let title = form.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_LEN || title.contains('\0') {
        return error_response(422, "Use a todo title between 1 and 160 characters.");
    }
    let mut store = state.store.lock().expect("store mutex poisoned");
    let id = store.next_todo_id;
    store.next_todo_id += 1;
    store.todos.push(Todo {
        id,
        owner_id: session.user_id,
        title: title.to_owned(),
        completed: false,
    });
    redirect("/todos")
}

async fn toggle_todo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<CsrfForm>,
) -> Response {
    let Some(session) = csrf_session(&state, &headers, &form.csrf) else {
        return error_response(403, "Invalid request token");
    };
    let mut store = state.store.lock().expect("store mutex poisoned");
    let Some(todo) = store
        .todos
        .iter_mut()
        .find(|todo| todo.id == id && todo.owner_id == session.user_id)
    else {
        return error_response(404, "Todo not found");
    };
    todo.completed = !todo.completed;
    redirect(&format!("/todos/{id}"))
}

async fn delete_todo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<CsrfForm>,
) -> Response {
    let Some(session) = csrf_session(&state, &headers, &form.csrf) else {
        return error_response(403, "Invalid request token");
    };
    let mut store = state.store.lock().expect("store mutex poisoned");
    let Some(index) = store
        .todos
        .iter()
        .position(|todo| todo.id == id && todo.owner_id == session.user_id)
    else {
        return error_response(404, "Todo not found");
    };
    store.todos.remove(index);
    redirect("/todos")
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    let Some(session_id) = session_cookie(&headers, state.config) else {
        return error_response(403, "Invalid request token");
    };
    let valid = state
        .store
        .lock()
        .expect("store mutex poisoned")
        .sessions
        .get(&session_id)
        .is_some_and(|session| token_eq(&session.csrf, &form.csrf));
    if !valid {
        return error_response(403, "Invalid request token");
    }
    state
        .store
        .lock()
        .expect("store mutex poisoned")
        .sessions
        .remove(&session_id);
    let mut response = redirect("/login");
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie(
            session_cookie_name(state.config),
            "",
            state.config,
            true,
            true,
        ),
    );
    response
}

fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).ok().is_some_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

fn take_login_csrf(state: &AppState, headers: &HeaderMap, supplied: &str) -> bool {
    cookie_value(headers, login_csrf_cookie_name(state.config))
        .is_some_and(|cookie_token| token_eq(&cookie_token, supplied))
        && state
            .store
            .lock()
            .expect("store mutex poisoned")
            .login_csrf
            .remove(supplied)
}

fn csrf_session(state: &AppState, headers: &HeaderMap, supplied: &str) -> Option<Session> {
    let session = current_session(state, headers)?;
    token_eq(&session.csrf, supplied).then_some(session)
}

fn current_session(state: &AppState, headers: &HeaderMap) -> Option<Session> {
    let id = session_cookie(headers, state.config)?;
    state
        .store
        .lock()
        .expect("store mutex poisoned")
        .sessions
        .get(&id)
        .cloned()
}

fn session_cookie(headers: &HeaderMap, config: AppConfig) -> Option<String> {
    cookie_value(headers, session_cookie_name(config))
}

fn session_cookie_name(config: AppConfig) -> &'static str {
    if config.secure_cookies {
        HOST_SESSION_COOKIE
    } else {
        LOCAL_SESSION_COOKIE
    }
}

fn login_csrf_cookie_name(config: AppConfig) -> &'static str {
    if config.secure_cookies {
        HOST_LOGIN_CSRF_COOKIE
    } else {
        LOCAL_LOGIN_CSRF_COOKIE
    }
}

fn cookie_value(headers: &HeaderMap, wanted: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (name == wanted).then(|| value.to_owned())
        })
}

fn token() -> String {
    OsRng
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

/// Tokens issued by this server are 48 bytes. Iterate that entire width even
/// when a hostile form value has another length.
fn token_eq(left: &str, right: &str) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..48 {
        difference |= usize::from(
            *left.as_bytes().get(index).unwrap_or(&0) ^ *right.as_bytes().get(index).unwrap_or(&0),
        );
    }
    difference == 0
}

fn cookie(name: &str, value: &str, config: AppConfig, http_only: bool, clear: bool) -> HeaderValue {
    let secure = config.secure_cookies.then_some("; Secure").unwrap_or("");
    let http_only = http_only.then_some("; HttpOnly").unwrap_or("");
    let expiry = clear.then_some("; Max-Age=0").unwrap_or("");
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; SameSite=Strict{secure}{http_only}{expiry}"
    ))
    .expect("cookie value is valid")
}

fn redirect(location: &str) -> Response {
    let mut response = Redirect::to(location).into_response();
    security_headers(response.headers_mut());
    response
}

fn page_response(status: StatusCode, html: String, set_cookie: Option<HeaderValue>) -> Response {
    let mut response = (status, Html(html)).into_response();
    if let Some(cookie) = set_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    security_headers(response.headers_mut());
    response
}

fn error_response(status: u16, message: &str) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    page_response(status, shell("Request note", &format!("<section class=\"notice\"><p class=\"eyebrow\">{}</p><h1>{}</h1><p><a href=\"/todos\">Return to your list</a></p></section>", status.as_u16(), escape(message))), None)
}

fn security_headers(headers: &mut HeaderMap) {
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
}

async fn add_security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    security_headers(response.headers_mut());
    response
}

fn user_name(store: &Store, id: u64) -> Option<&'static str> {
    store
        .users
        .iter()
        .find(|user| user.id == id)
        .map(|user| user.username)
}

fn login_html(csrf: &str, error: Option<&str>) -> String {
    let error = error
        .map(|message| format!("<p class=\"alert\" role=\"alert\">{}</p>", escape(message)))
        .unwrap_or_default();
    shell(
        "Sign in",
        &format!(
            r#"<section class="login-sheet" aria-labelledby="title"><p class="eyebrow">Private field notes · 01</p><h1 id="title">Make a small,<br><em>useful</em> list.</h1><p class="lede">A deliberately tiny server-rendered todo desk. Your work stays separate from every other account.</p>{error}<form method="post" action="/login" class="stack"><input type="hidden" name="csrf" value="{}"><label>Username<input name="username" autocomplete="username" required maxlength="64"></label><label>Password<input name="password" type="password" autocomplete="current-password" required maxlength="256"></label><button type="submit">Enter the desk <span aria-hidden="true">→</span></button></form><p class="hint">Demo accounts: <code>alice</code> / <code>demo-password</code>, or <code>bob</code> / <code>demo-password</code>.</p></section>"#,
            escape_attr(csrf)
        ),
    )
}

fn todos_html(user: &str, csrf: &str, todos: &[&Todo]) -> String {
    let items = if todos.is_empty() {
        "<li class=\"empty\">No open loops. Add one worth keeping.</li>".into()
    } else {
        todos
            .iter()
            .map(|todo| todo_row(todo, csrf))
            .collect::<String>()
    };
    shell(
        "Your desk",
        &format!(
            r#"<header class="masthead"><a class="wordmark" href="/todos">MARGIN <i>notes</i></a><form method="post" action="/logout"><input type="hidden" name="csrf" value="{}"><button class="quiet" type="submit">Sign out</button></form></header><main class="desk" aria-labelledby="title"><div class="chapter"><p class="eyebrow">{}'s field notes · {}</p><h1 id="title">Today's<br><em>small moves.</em></h1></div><section class="new-note" aria-labelledby="new-title"><h2 id="new-title">Add a note</h2><form method="post" action="/todos" class="add-form"><input type="hidden" name="csrf" value="{}"><label class="sr-only" for="title">Todo title</label><input id="title" name="title" maxlength="160" required placeholder="Name the next useful thing"><button type="submit">Pin it <span aria-hidden="true">↗</span></button></form></section><section aria-labelledby="list-title"><div class="section-line"><h2 id="list-title">Pinned work</h2><span>{} item{}</span></div><ol class="todo-list">{items}</ol></section></main>"#,
            escape_attr(csrf),
            escape(user),
            todos.len(),
            escape_attr(csrf),
            todos.len(),
            if todos.len() == 1 { "" } else { "s" }
        ),
    )
}

fn todo_row(todo: &Todo, csrf: &str) -> String {
    let state = if todo.completed { "done" } else { "" };
    let mark = if todo.completed { "✓" } else { "" };
    format!(
        r#"<li class="todo {state}"><form method="post" action="/todos/{}/toggle"><input type="hidden" name="csrf" value="{}"><button class="check" type="submit" aria-label="Mark {} {}">{mark}</button></form><a href="/todos/{}">{}</a><span class="status">{}</span></li>"#,
        todo.id,
        escape_attr(csrf),
        escape_attr(&todo.title),
        if todo.completed {
            "unfinished"
        } else {
            "complete"
        },
        todo.id,
        escape(&todo.title),
        if todo.completed { "Filed" } else { "In play" }
    )
}

fn detail_html(todo: &Todo, csrf: &str) -> String {
    let status = if todo.completed { "Filed" } else { "In play" };
    let action = if todo.completed {
        "Restore to desk"
    } else {
        "Mark as filed"
    };
    shell(
        "Todo detail",
        &format!(
            r#"<header class="masthead"><a class="wordmark" href="/todos">MARGIN <i>notes</i></a><a class="quiet" href="/todos">← All notes</a></header><main class="detail" aria-labelledby="title"><p class="eyebrow">Note no. {:03} · {status}</p><h1 id="title">{}</h1><div class="rule"></div><p class="lede">One thing, clearly named. Keep the list light enough to move.</p><div class="actions"><form method="post" action="/todos/{}/toggle"><input type="hidden" name="csrf" value="{}"><button type="submit">{action}</button></form><form method="post" action="/todos/{}/delete"><input type="hidden" name="csrf" value="{}"><button class="danger" type="submit">Remove note</button></form></div></main>"#,
            todo.id,
            escape(&todo.title),
            todo.id,
            escape_attr(csrf),
            todo.id,
            escape_attr(csrf)
        ),
    )
}

fn shell(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{} · Margin Notes</title><style>{}</style></head><body><div class="grain" aria-hidden="true"></div><div class="page">{body}</div></body></html>"#,
        escape(title),
        CSS
    )
}

fn escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&#x27;".chars().collect(),
            _ => vec![ch],
        })
        .collect()
}
fn escape_attr(value: &str) -> String {
    escape(value)
}

const CSS: &str = r#"
:root{--ink:#1c211b;--paper:#f4f0e5;--orange:#e45c2b;--line:#cfc6b5;--soft:#e5decc}*{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);font-family:'DM Mono',monospace}.grain{position:fixed;inset:0;pointer-events:none;opacity:.22;background-image:radial-gradient(#253022 0.55px,transparent .7px);background-size:5px 5px}.page{position:relative;max-width:1120px;margin:auto;padding:30px 48px 80px}.masthead{display:flex;align-items:center;justify-content:space-between;padding:8px 0 28px;border-bottom:1px solid var(--ink)}.wordmark{color:inherit;text-decoration:none;font-size:18px;letter-spacing:-1px}.wordmark i{font-family:Fraunces,serif;font-weight:500}.quiet{color:inherit;background:none;border:0;font:inherit;font-size:12px;text-decoration:none;cursor:pointer;padding:8px 0}.desk{display:grid;grid-template-columns:minmax(250px,.9fr) minmax(360px,1.4fr);gap:72px;padding-top:70px}.chapter{padding-top:13px}.eyebrow{font-size:11px;letter-spacing:.06em;text-transform:uppercase;margin:0 0 18px;color:#5f6257}.desk h1,.login-sheet h1,.detail h1{font:700 clamp(42px,7vw,86px)/.91 Fraunces,Georgia,serif;letter-spacing:-.065em;margin:0}.desk h1 em,.login-sheet h1 em{font-weight:500}.new-note{border-top:3px solid var(--ink);padding-top:18px;margin-bottom:52px}h2{font-size:12px;text-transform:uppercase;letter-spacing:.08em;margin:0 0 13px}.add-form{display:flex;border-bottom:1px solid var(--ink)}input{background:transparent;border:0;border-radius:0;color:var(--ink);font:14px 'DM Mono',monospace;min-width:0;padding:15px 12px;outline-offset:4px}.add-form input{width:100%}button{border:1px solid var(--ink);background:var(--ink);color:var(--paper);font:500 12px 'DM Mono',monospace;cursor:pointer;padding:13px 16px;white-space:nowrap;transition:transform .15s,background .15s}button:hover{background:var(--orange);transform:translate(-2px,-2px);box-shadow:3px 3px 0 var(--ink)}button:focus-visible,a:focus-visible,input:focus-visible{outline:3px solid var(--orange);outline-offset:3px}.section-line{display:flex;justify-content:space-between;align-items:baseline;border-bottom:1px solid var(--line);padding-bottom:12px}.section-line span,.status,.hint{font-size:11px;color:#68685d}.todo-list{list-style:none;padding:0;margin:0}.todo{display:grid;grid-template-columns:32px 1fr auto;align-items:center;gap:12px;border-bottom:1px solid var(--line);min-height:66px}.todo>a{color:var(--ink);font:500 15px Fraunces,Georgia,serif;text-decoration:none}.todo>a:hover{text-decoration:underline;text-decoration-color:var(--orange);text-decoration-thickness:2px}.check{width:21px;height:21px;padding:0;border-radius:50%;color:var(--ink);background:transparent;box-shadow:none}.check:hover{background:var(--orange);color:white;box-shadow:none;transform:none}.done a{text-decoration:line-through;color:#77766d}.done .check{background:var(--ink);color:var(--paper)}.empty{padding:30px 0;font:italic 18px Fraunces,serif}.login-sheet{max-width:570px;margin:8vh auto 0;border-top:3px solid var(--ink);padding-top:25px}.login-sheet .lede,.detail .lede{font:18px/1.45 Fraunces,Georgia,serif;max-width:470px;margin:28px 0}.stack{display:grid;gap:20px;margin:32px 0}.stack label{display:grid;gap:8px;font-size:11px;text-transform:uppercase;letter-spacing:.06em}.stack input{border-bottom:1px solid var(--ink);padding:12px 0}.stack button{justify-self:start}.hint{line-height:1.7}.hint code{color:var(--ink)}.alert{border-left:4px solid var(--orange);padding:10px 12px;background:#f6d7c8;font-size:12px}.detail{max-width:760px;margin:11vh auto 0}.detail h1{max-width:740px}.rule{width:75px;height:5px;background:var(--orange);margin:30px 0}.actions{display:flex;gap:12px;margin-top:40px}.danger{background:transparent;color:var(--ink)}.danger:hover{color:white}.notice{max-width:550px;margin:16vh auto 0;border-top:3px solid var(--ink);padding-top:24px}.notice h1{font:700 52px/.95 Fraunces,serif;margin:0}.notice a{color:var(--orange)}.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}@media(max-width:680px){.page{padding:22px 22px 56px}.desk{grid-template-columns:1fr;gap:48px;padding-top:45px}.desk h1{font-size:60px}.login-sheet,.detail{margin-top:8vh}.todo{grid-template-columns:30px 1fr}.todo .status{display:none}.actions{flex-wrap:wrap}}@media(prefers-reduced-motion:reduce){*{transition:none!important}}
"#;
