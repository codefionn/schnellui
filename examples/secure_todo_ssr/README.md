# Secure Todo SSR

A small server-rendered Axum example demonstrating how `schnellui-render-html`'s `HtmlRouter` and mandatory `SsrAuthorize` checks can guard native pages. `GET /todos` requires a signed-in session and `GET /todos/:id` additionally checks that the todo belongs to that session's user.

## Run

```bash
TODO_SECURE_COOKIES=0 cargo run -p secure_todo_ssr
```

Open <http://127.0.0.1:3000>. Use `alice` / `demo-password` or `bob` / `demo-password`. Set `TODO_SECURE_COOKIES=1` (the default) behind HTTPS. The `=0` setting is only for local HTTP development and automated HTTP tests.

The example uses Argon2 password hashes, opaque randomly generated server-side sessions, `HttpOnly; SameSite=Strict` cookies, CSRF tokens on login and every mutation, owner-scoped todo operations, escaping, and restrictive security headers.

Run the HTTP-level authorization suite with:

```bash
cargo test -p secure_todo_ssr --test authorization
```

## Production limits

This is a teaching example, not a deployment recipe. Its mutex-protected state, sessions, CSRF tokens, and demo password hashes disappear on restart and do not work across replicas. It has no rate limiting, account lifecycle, session expiry, audit logs, persistent database, TLS termination, observability, or key management. Put it behind HTTPS, use a real database/session store, add expiry and throttling, and replace the demo identities before shipping.
