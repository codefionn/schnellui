# Native HTML SSR router

This is the separate native server crate for the router example. It depends on
`html_native_router` only for the shared hydration contract and view, then owns:

- the request context and server-only secret;
- the mandatory `SsrAuthorize` implementation;
- the `SsrRoute` implementation and chained SSR stages;
- the explicit selection of state hydrated to the CSR client.

Build the separate CSR bundle:

```sh
cd examples/html_native_router
NO_COLOR=true RUSTUP_TOOLCHAIN=stable trunk build
cd ../..
```

Start the SSR web server:

```sh
cargo run -p html_native_router_ssr -- --serve
```

Then open <http://127.0.0.1:8080/users/42/dashboard>. `--serve` SSR-renders and
authorizes route requests, injects the CSR bootstrap from the Trunk output, and
serves the hashed JS/WASM assets itself.

Useful options:

```text
--address 127.0.0.1:9000
--client-dir /path/to/trunk/dist
```

Without `--serve`, the crate still supports deterministic one-shot rendering:

```sh
cargo run -p html_native_router_ssr -- --out page.html
```
