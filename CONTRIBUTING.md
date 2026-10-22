# Contributing to Cray

Thanks for your interest in contributing! 🎉

## Getting Started

1. Fork the repository
2. Create a branch for your feature or fix:
   ```bash
   git checkout -b feature/my-feature
   ```
3. Make your changes and commit them:
   ```bash
   git commit -m "Add my feature"
   ```
4. Push the branch:
   ```bash
   git push origin feature/my-feature
   ```
5. Open a Pull Request

## Development Environment

Building requires Rust, `cmake`, and a C/C++ compiler (TLS is built on BoringSSL through `btls`).

```bash
cargo build --release
cargo test --workspace
```

## Code Style

- Formatting is enforced with `rustfmt` — run `cargo fmt --all` before committing
- Lints are enforced with `clippy` — run `cargo clippy --workspace` to keep warnings at zero
- CI runs both checks on every push, so the PR will be green only if these pass

## PR Guidelines

- Keep PRs focused: one feature or fix per PR
- Add or update tests for changes where it makes sense
- Update the README (and `README.ru.md`) if your change affects the docs

## License

By contributing, you agree that your contributions will be licensed under the [GPL-2.0](./LICENSE) license.

---

See the list of contributors in [CONTRIBUTORS.md](./CONTRIBUTORS.md).
