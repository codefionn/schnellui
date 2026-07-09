# Native HTML SSR/CSR router

The route example is split at the real deployment seam:

- `html_native_router_ssr` is the native SSR server crate;
- `html_native_router` is the WASM CSR client and shared hydration-contract
  library.

Together they implement:

```text
authorized outer SSR route
└── hydrated CSR route
    └── nested server-derived view
```

The SSR crate implements `SsrAuthorize` and `SsrRoute`. It chains request
data into server state, retains a database password only on the server, and
serializes an explicitly selected `ClientState`. The WASM target consumes that
state and mounts the matching `CsrRoute`; its counter updates without reloading
the page, and the router handles internal links and browser history.

Build the CSR bundle used by the SSR server:

```sh
cd examples/html_native_router
NO_COLOR=true RUSTUP_TOOLCHAIN=stable trunk build
cd ../..
```

Start the SSR server:

```sh
cargo run -p html_native_router_ssr -- --serve
```

Open <http://127.0.0.1:8080/users/42/dashboard>. The server renders and
authorizes that request, injects the Trunk bootstrap, and serves the CSR assets.
