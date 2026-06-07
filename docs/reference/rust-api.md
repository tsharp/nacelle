# Rust API reference

Generate the Rust API reference with:

```bash
cargo doc --workspace --all-features --no-deps
```

On Windows:

```powershell
.\scripts\build-rustdoc.ps1
```

The generated index is:

```text
target/doc/nacelle/index.html
```

