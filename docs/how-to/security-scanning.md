# Run security scans

Run vulnerability and dependency checks before release:

```bash
cargo audit
cargo tree -i serde_yaml
cargo tree -i unsafe-libyaml
```

`serde_yaml` and `unsafe-libyaml` should not appear in the dependency tree. If `cargo-deny` is adopted, add `deny.toml` with accepted licenses, advisory exceptions, and source policy.


