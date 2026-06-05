# Versioning Control System (VCS) Guidelines

- Main Branch: `main`
- Platform: `github` (repo: `RebelliousSmile/WinFXStart`)
- CLI: `gh`

## Branch Naming Convention

### Format

```text
type/short-description
```

### Types

| Prefix       | Usage                     |
| ------------ | ------------------------- |
| `feat/`      | New feature               |
| `fix/`       | Bug fix                   |
| `docs/`      | Documentation only        |
| `refactor/`  | Code change (no feat/fix) |
| `chore/`     | Build, config, deps       |
| `test/`      | Add/update tests          |
| `hotfix/`    | Urgent production fix     |

### Examples

```text
feat/registry-startup
fix/process-spawn-quoting
docs/update-roadmap
refactor/extract-json-storage
chore/bump-windows-crate
```

## Commit Convention

### Format

```text
type(scope): description

[optional body]

[optional footer]
```

### Types

| Type       | Usage                        |
| ---------- | ---------------------------- |
| `feat`     | New feature                  |
| `fix`      | Bug fix                      |
| `docs`     | Documentation only           |
| `refactor` | Code change (no feat/fix)    |
| `perf`     | Performance improvement      |
| `test`     | Add/update tests             |
| `chore`    | Build, config, deps          |
| `style`    | Formatting (no logic change) |
| `ci`       | CI/CD configuration          |
| `revert`   | Revert previous commit       |

### Description rules

- Imperative mood: "add" not "added"
- Lowercase, no period
- Max 72 chars

### Examples

```text
feat(startup): register app via Registry Run Keys
fix(exec): handle paths with spaces
docs(readme): clarify build prerequisites
```
