# Coding Guidelines

> Those rules must be minimal because they MUST be checked after EVERY CODE GENERATION.

## Requirements to complete a feature

**A feature is really completed if ALL of the below are satisfied: if not, iterate to fix all until all are green.**

## Commands to run

- `Before commit`: minimal check to build a feature
- `Before push`: heavier check ran before push

### Before commit

```markdown
| Order | Command              | Description                          |
| ----- | -------------------- | ------------------------------------ |
| 1     | `cargo check`        | Type/compile check                   |
| 2     | `cargo clippy`       | Lint warnings                        |
| 3     | `cargo fmt --check`  | Formatting check (rustfmt)           |
```

### Before push

```markdown
| Order | Command                  | Description                     |
| ----- | ------------------------ | ------------------------------- |
| 1     | `cargo test`             | Unit/integration tests          |
| 2     | `cargo build --release`  | Release build (lto, opt-level 3)|
```
