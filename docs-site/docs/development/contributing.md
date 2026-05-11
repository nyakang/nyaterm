---
sidebar_position: 5
---

# 贡献指南

感谢你有兴趣为 NyaTerm 做出贡献！

## 开始之前

1. 确保你已经阅读了 [开发环境搭建](./setup) 文档
2. 查看 [Issues](https://github.com/nyakang/nyaterm/issues) 列表，了解当前的任务和 Bug

## 贡献流程

1. **Fork 仓库** — 在 Git 平台上 Fork 项目
2. **创建分支** — 基于 `main` 创建功能分支
   ```bash
   git checkout -b feat/my-feature
   ```
3. **开发** — 编写代码并测试
4. **提交** — 使用规范的提交信息
5. **推送** — 推送到你的 Fork
6. **创建 PR** — 提交 Pull Request

## 提交规范

使用 [Conventional Commits](https://www.conventionalcommits.org/) 格式：

```
<type>(<scope>): <description>
```

常用类型：

| 类型 | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `docs` | 文档更新 |
| `style` | 代码格式（不影响逻辑） |
| `refactor` | 代码重构 |
| `perf` | 性能优化 |
| `chore` | 构建/工具变更 |

示例：

```
feat(sftp): add batch file download support
fix(ssh): handle connection timeout correctly
docs: update installation guide
```

## 代码规范

### 前端

- 使用 TypeScript 严格模式
- 运行 `pnpm lint` 确保通过代码检查
- 运行 `pnpm format` 格式化代码
- 使用函数组件和 Hooks

### 后端

- 遵循 Rust 标准编码风格
- 使用 `cargo clippy` 检查代码
- 使用 `cargo fmt` 格式化代码
- 合理使用错误处理，避免 `unwrap()`

## 国际化

添加或修改 UI 文本时，请同时更新：

- `src/i18n/locales/zh-CN.json` — 简体中文
- `src/i18n/locales/en.json` — English

## 许可证

贡献的代码将遵循项目的 [MIT 许可证](https://github.com/nyakang/nyaterm/blob/main/LICENSE)。
