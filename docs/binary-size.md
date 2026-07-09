# Binary size

Debug info is most of a schnellui binary. A debug build of scriptschnell-term was
1.19 GB with cargo's defaults and 139 MB with the profiles below. Its release build
went down to 67 MB. Test executables shrink the same way, so `target/` grows far
slower.

Cargo ignores the profiles of dependencies, so an app that uses schnellui has to set
these in its own workspace `Cargo.toml`. This workspace uses the same ones.

```toml
# Your own crates keep line tables, so panics and backtraces name files and lines.
[profile.dev]
debug = "line-tables-only"

# Dependencies carry no debug info.
[profile.dev.package."*"]
debug = false

[profile.release]
lto = "thin"
codegen-units = 1
debug = false
# Keeps symbol names, so backtraces name functions. `strip = true` saves about
# another 12% but drops them.
strip = "debuginfo"
```

To see local variables in a debugger, turn full debug info back on for one build:
`cargo build --config 'profile.dev.debug=true' --config 'profile.dev.package."*".debug=true'`.
