---
sidebar_position: 5
---

# Contributing

Thank you for your interest in contributing to NyaTerm!

## Before You Start

1. Read the [Development Setup](./setup) documentation
2. Check the [Issues](https://github.com/nyakang/nyaterm/issues) list

## Contribution Workflow

1. **Fork the repository**
2. **Create a branch** from `main`
   ```bash
   git checkout -b feat/my-feature
   ```
3. **Develop** — Write code and test
4. **Commit** — Use conventional commit messages
5. **Push** — Push to your fork
6. **Create PR** — Submit a Pull Request

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>
```

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation |
| `style` | Code formatting (no logic change) |
| `refactor` | Code refactoring |
| `perf` | Performance improvement |
| `chore` | Build/tooling changes |

Examples:

```
feat(sftp): add batch file download support
fix(ssh): handle connection timeout correctly
docs: update installation guide
```

## Code Standards

### Frontend

- TypeScript strict mode
- Run `pnpm lint` to pass checks
- Run `pnpm format` to format code
- Use functional components and Hooks

### Backend

- Follow standard Rust coding style
- Run `cargo clippy` for linting
- Run `cargo fmt` for formatting
- Use proper error handling, avoid `unwrap()`

## Internationalization

When adding or modifying UI text, update both:

- `src/i18n/locales/zh-CN.json`
- `src/i18n/locales/en.json`

## License

Contributions are licensed under the project's [MIT License](https://github.com/nyakang/nyaterm/blob/main/LICENSE).
