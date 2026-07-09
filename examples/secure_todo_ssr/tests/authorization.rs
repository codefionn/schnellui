use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::Response,
    Router,
};
use secure_todo_ssr::{app, AppConfig};
use tower::ServiceExt;

#[derive(Clone, Debug)]
struct Cookie {
    name: String,
    value: String,
    raw: String,
}

impl Cookie {
    fn pair(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

#[derive(Clone, Debug)]
struct SignedIn {
    session: Cookie,
    csrf: String,
}

fn production_app() -> Router {
    // Requests are in-process, so the Secure cookie is deliberately supplied
    // by the test client rather than relying on a browser cookie jar.
    app(AppConfig {
        secure_cookies: true,
    })
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    form: Option<String>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let body = if let Some(form) = form {
        builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        Body::from(form)
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).expect("valid test request"))
        .await
        .expect("router responds")
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body is readable")
            .to_vec(),
    )
    .expect("HTML is utf-8")
}

fn location(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
}

fn assert_security_headers(headers: &HeaderMap) {
    assert_eq!(
        headers.get(header::CONTENT_SECURITY_POLICY).and_then(|value| value.to_str().ok()),
        Some("default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'")
    );
    assert_eq!(
        headers
            .get(header::X_FRAME_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("DENY")
    );
    assert_eq!(
        headers
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        headers
            .get(header::REFERRER_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some("no-referrer")
    );
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store, max-age=0")
    );
}

fn cookie_from_headers(headers: &HeaderMap, matches: impl Fn(&str) -> bool) -> Cookie {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|raw| {
            let first = raw.split(';').next()?;
            let (name, value) = first.split_once('=')?;
            matches(name).then(|| Cookie {
                name: name.to_owned(),
                value: value.to_owned(),
                raw: raw.to_owned(),
            })
        })
        .expect("expected Set-Cookie header")
}

fn session_cookie(headers: &HeaderMap) -> Cookie {
    cookie_from_headers(headers, |name| {
        name.contains("todo") && !name.contains("csrf")
    })
}

fn assert_only_csrf_cookies(headers: &HeaderMap) {
    assert!(
        headers.get_all(header::SET_COOKIE).iter().all(|value| {
            value
                .to_str()
                .ok()
                .and_then(|raw| raw.split(';').next())
                .and_then(|first| first.split_once('='))
                .is_some_and(|(name, _)| name.contains("csrf"))
        }),
        "failed login must not issue a session cookie"
    );
}

fn login_csrf_cookie(headers: &HeaderMap) -> Cookie {
    cookie_from_headers(headers, |name| {
        name.contains("login") && name.contains("csrf")
    })
}

fn attribute(tag: &str, wanted: &str) -> Option<String> {
    let needle = format!("{}=\"", wanted);
    let value = tag.split_once(&needle)?.1;
    Some(value.split_once('"')?.0.to_owned())
}

fn hidden_field(page: &str, wanted: &str) -> String {
    page.split("<input")
        .filter_map(|tail| tail.split_once('>').map(|(tag, _)| tag))
        .find_map(|tag| {
            (attribute(tag, "name").as_deref() == Some(wanted))
                .then_some(tag)
                .and_then(|tag| attribute(tag, "value"))
        })
        .expect("hidden field is rendered")
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{}={}", url_encode(name), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

async fn sign_in(app: &Router, username: &str) -> SignedIn {
    sign_in_with_existing_session(app, username, None).await
}

async fn sign_in_with_existing_session(
    app: &Router,
    username: &str,
    existing_session: Option<&Cookie>,
) -> SignedIn {
    let login = request(app, Method::GET, "/login", None, None).await;
    assert_eq!(login.status(), StatusCode::OK);
    let login_headers = login.headers().clone();
    let csrf_cookie = login_csrf_cookie(&login_headers);
    let csrf = hidden_field(&html(login).await, "csrf");
    assert_eq!(
        csrf_cookie.value, csrf,
        "login CSRF cookie and form must agree"
    );

    let cookies = existing_session
        .map(|session| format!("{}; {}", session.pair(), csrf_cookie.pair()))
        .unwrap_or_else(|| csrf_cookie.pair());
    let login = request(
        app,
        Method::POST,
        "/login",
        Some(&cookies),
        Some(form(&[
            ("username", username),
            ("password", "demo-password"),
            ("csrf", &csrf),
        ])),
    )
    .await;
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(login.headers()), Some("/todos"));
    let session = session_cookie(login.headers());

    let todos = request(app, Method::GET, "/todos", Some(&session.pair()), None).await;
    assert_eq!(todos.status(), StatusCode::OK);
    let csrf = hidden_field(&html(todos).await, "csrf");
    SignedIn { session, csrf }
}

async fn signed_in_page(app: &Router, user: &SignedIn, path: &str) -> String {
    let response = request(app, Method::GET, path, Some(&user.session.pair()), None).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{path} should be accessible to its owner"
    );
    html(response).await
}

#[tokio::test]
async fn unauthenticated_and_login_failures_do_not_create_a_session() {
    let app = production_app();

    for path in ["/todos", "/todos/1"] {
        let response = request(&app, Method::GET, path, None, None).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(response.headers()), Some("/login"));
        assert_security_headers(response.headers());
        let page = html(response).await;
        assert!(!page.contains("Read the field notes"));
        assert!(!page.contains("Calibrate the tiny telescope"));
    }

    let missing_csrf = request(
        &app,
        Method::POST,
        "/login",
        None,
        Some(form(&[
            ("username", "alice"),
            ("password", "demo-password"),
            ("csrf", "not-issued"),
        ])),
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::UNAUTHORIZED);
    assert_only_csrf_cookies(missing_csrf.headers());

    let login = request(&app, Method::GET, "/login", None, None).await;
    let login_cookie = login_csrf_cookie(login.headers());
    let csrf = hidden_field(&html(login).await, "csrf");
    let bad_password = request(
        &app,
        Method::POST,
        "/login",
        Some(&login_cookie.pair()),
        Some(form(&[
            ("username", "alice"),
            ("password", "wrong-password"),
            ("csrf", &csrf),
        ])),
    )
    .await;
    assert_eq!(bad_password.status(), StatusCode::UNAUTHORIZED);
    assert_only_csrf_cookies(bad_password.headers());
}

#[tokio::test]
async fn logins_rotate_secure_sessions_and_isolate_each_users_todos() {
    let app = production_app();
    let alice_first = sign_in(&app, "alice").await;
    let alice_rotated =
        sign_in_with_existing_session(&app, "alice", Some(&alice_first.session)).await;
    let bob = sign_in(&app, "bob").await;

    assert_ne!(
        alice_first.session.value, alice_rotated.session.value,
        "a new login rotates the session id"
    );
    let fixed_session = request(
        &app,
        Method::GET,
        "/todos",
        Some(&alice_first.session.pair()),
        None,
    )
    .await;
    assert_eq!(
        fixed_session.status(),
        StatusCode::SEE_OTHER,
        "a supplied pre-login session must be invalidated"
    );
    assert_eq!(location(fixed_session.headers()), Some("/login"));
    assert_ne!(
        alice_rotated.session.value, bob.session.value,
        "sessions are not shared between users"
    );
    assert!(
        alice_first.session.name.starts_with("__Host-"),
        "production session cookie must use the __Host- prefix"
    );
    for attribute in ["Path=/", "SameSite=Strict", "Secure", "HttpOnly"] {
        assert!(
            alice_first.session.raw.contains(attribute),
            "session cookie must include {attribute}: {}",
            alice_first.session.raw
        );
    }
    assert!(
        !alice_first.session.raw.contains("Domain="),
        "__Host- cookies cannot set Domain"
    );

    let alice_page = signed_in_page(&app, &alice_rotated, "/todos").await;
    assert!(alice_page.contains("Read the field notes"));
    assert!(alice_page.contains("Send the Monday dispatch"));
    assert!(!alice_page.contains("Calibrate the tiny telescope"));

    let bob_page = signed_in_page(&app, &bob, "/todos").await;
    assert!(bob_page.contains("Calibrate the tiny telescope"));
    assert!(!bob_page.contains("Read the field notes"));
    assert!(!bob_page.contains("Send the Monday dispatch"));
}

#[tokio::test]
async fn ownership_and_csrf_protect_every_mutation_and_logout() {
    let app = production_app();
    let alice = sign_in(&app, "alice").await;
    let bob = sign_in(&app, "bob").await;

    let response = request(
        &app,
        Method::GET,
        "/todos/3",
        Some(&alice.session.pair()),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!html(response)
        .await
        .contains("Calibrate the tiny telescope"));

    for endpoint in ["/todos/3/toggle", "/todos/3/delete"] {
        let response = request(
            &app,
            Method::POST,
            endpoint,
            Some(&alice.session.pair()),
            Some(form(&[("csrf", &alice.csrf)])),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "Alice must not mutate Bob's known id"
        );
    }
    let bob_detail = signed_in_page(&app, &bob, "/todos/3").await;
    assert!(bob_detail.contains("Calibrate the tiny telescope"));
    assert!(
        bob_detail.contains("In play"),
        "Alice's rejected toggle cannot change Bob's todo"
    );

    for endpoint in ["/todos/1/toggle", "/todos/2/delete"] {
        for request_form in [String::new(), form(&[("csrf", "wrong")])] {
            let response = request(
                &app,
                Method::POST,
                endpoint,
                Some(&alice.session.pair()),
                Some(request_form),
            )
            .await;
            assert!(matches!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY | StatusCode::FORBIDDEN
            ));
        }
    }
    assert!(signed_in_page(&app, &alice, "/todos/1")
        .await
        .contains("In play"));
    assert!(signed_in_page(&app, &alice, "/todos/2")
        .await
        .contains("Send the Monday dispatch"));

    for request_form in [
        form(&[("title", "missing token")]),
        form(&[("title", "wrong token"), ("csrf", "wrong")]),
        form(&[("title", "cross-session token"), ("csrf", &bob.csrf)]),
    ] {
        let response = request(
            &app,
            Method::POST,
            "/todos",
            Some(&alice.session.pair()),
            Some(request_form),
        )
        .await;
        assert!(matches!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY | StatusCode::FORBIDDEN
        ));
    }
    let alice_page = signed_in_page(&app, &alice, "/todos").await;
    for title in ["missing token", "wrong token", "cross-session token"] {
        assert!(!alice_page.contains(title));
    }

    let add = request(
        &app,
        Method::POST,
        "/todos",
        Some(&alice.session.pair()),
        Some(form(&[("title", "CSRF accepted"), ("csrf", &alice.csrf)])),
    )
    .await;
    assert_eq!(add.status(), StatusCode::SEE_OTHER);
    assert!(signed_in_page(&app, &alice, "/todos")
        .await
        .contains("CSRF accepted"));

    let toggle = request(
        &app,
        Method::POST,
        "/todos/1/toggle",
        Some(&alice.session.pair()),
        Some(form(&[("csrf", &alice.csrf)])),
    )
    .await;
    assert_eq!(toggle.status(), StatusCode::SEE_OTHER);
    assert!(signed_in_page(&app, &alice, "/todos/1")
        .await
        .contains("Filed"));

    let delete = request(
        &app,
        Method::POST,
        "/todos/4/delete",
        Some(&alice.session.pair()),
        Some(form(&[("csrf", &alice.csrf)])),
    )
    .await;
    assert_eq!(delete.status(), StatusCode::SEE_OTHER);
    let deleted = request(
        &app,
        Method::GET,
        "/todos/4",
        Some(&alice.session.pair()),
        None,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NOT_FOUND);

    let missing_logout_csrf = request(
        &app,
        Method::POST,
        "/logout",
        Some(&alice.session.pair()),
        Some(String::new()),
    )
    .await;
    assert!(matches!(
        missing_logout_csrf.status(),
        StatusCode::UNPROCESSABLE_ENTITY | StatusCode::FORBIDDEN
    ));
    assert_eq!(
        request(
            &app,
            Method::GET,
            "/todos",
            Some(&alice.session.pair()),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );

    let logout = request(
        &app,
        Method::POST,
        "/logout",
        Some(&alice.session.pair()),
        Some(form(&[("csrf", &alice.csrf)])),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::SEE_OTHER);
    assert!(session_cookie(logout.headers()).raw.contains("Max-Age=0"));
    let after_logout = request(
        &app,
        Method::GET,
        "/todos",
        Some(&alice.session.pair()),
        None,
    )
    .await;
    assert_eq!(after_logout.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(after_logout.headers()), Some("/login"));
}

#[tokio::test]
async fn post_only_endpoints_escape_xss_and_validate_titles() {
    let app = production_app();
    let alice = sign_in(&app, "alice").await;

    let get_toggle = request(
        &app,
        Method::GET,
        "/todos/1/toggle",
        Some(&alice.session.pair()),
        None,
    )
    .await;
    assert_eq!(get_toggle.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_security_headers(get_toggle.headers());
    assert!(signed_in_page(&app, &alice, "/todos/1")
        .await
        .contains("In play"));

    for title in ["", &"x".repeat(161)] {
        let response = request(
            &app,
            Method::POST,
            "/todos",
            Some(&alice.session.pair()),
            Some(form(&[("title", title), ("csrf", &alice.csrf)])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    let xss = "<script>alert('owned')</script>";
    let create = request(
        &app,
        Method::POST,
        "/todos",
        Some(&alice.session.pair()),
        Some(form(&[("title", xss), ("csrf", &alice.csrf)])),
    )
    .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);
    let page = signed_in_page(&app, &alice, "/todos").await;
    assert!(page.contains("&lt;script&gt;alert(&#x27;owned&#x27;)&lt;/script&gt;"));
    assert!(!page.contains(xss));
}
