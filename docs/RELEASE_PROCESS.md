# Veya 发布操作指南

本文记录完整的版本发布流程，包含本地准备、CI 等待、标签推送和发布验证。每次发布前阅读本文，按步骤执行。

## 前置条件

- 所有功能变更已合并到 `main` 分支
- 本地已安装 Rust 1.99.0 MSVC 工具链、NSIS 和 GitHub CLI（`gh`）
- GitHub 仓库 `NGLSL/Veya` 可访问

## 步骤一：更新版本号

更新以下文件中的版本号（示例：`0.2.5` → `0.2.6`）：

1. `Cargo.toml` — `[workspace.package] version`
2. `Cargo.lock` — 4 个 workspace members 的 `version` 字段：
   - `veya-core`
   - `veya-desktop`
   - `veya-storage`
   - `veya-windows`

> **注意**：Cargo.lock 中第三方依赖（如 `piper`、`tiny-xlib`、`wasm-timer`）可能恰好版本号相同，不要修改它们。只修改没有 `source = "registry+..."` 字段的本地包。

## 步骤二：编写发布说明

创建 `docs/releases/vX.Y.Z.md`（示例：`docs/releases/v0.2.6.md`）。

格式参考 `docs/releases/v0.2.5.md`，包含以下章节：

```markdown
# Veya vX.Y.Z

一句话概述本版本核心变化。

## [功能分类标题]

- 具体变化描述

## 构建与升级

- 是否可覆盖安装
- 数据库迁移说明
- 数据兼容性

## 验证

- 本地验证结果
- CI 验证范围

## 已知限制

- 已知问题或后续计划
```

## 步骤三：更新文档

根据需要更新以下文件：

- `README.md` — 核心能力描述中的新增功能
- `docs/ARCHITECTURE.md` — 架构变化（如有）
- `docs/DEVELOPMENT.md` — 开发流程变化（如有）

## 步骤四：本地验证

在仓库根目录执行：

```powershell
cargo fmt --all -- --check
cargo check --locked --workspace
cargo test --locked --workspace
```

本地构建安装包（需要 NSIS）：

```powershell
./scripts/build-installer.ps1 -Nsis 'C:\Program Files (x86)\NSIS\makensis.exe'
```

## 步骤五：提交并推送

```powershell
git add Cargo.toml Cargo.lock docs/releases/vX.Y.Z.md README.md docs/ARCHITECTURE.md docs/DEVELOPMENT.md
git commit -m "release: publish Veya vX.Y.Z with [feature summary]"
git push origin main
```

## 步骤六：等待 CI 成功

在 GitHub Actions 页面确认 CI 工作流（`ci.yml`）在 `main` 分支的最新提交上运行成功。

CI 执行内容：

1. 安装 Rust 1.99.0 MSVC 工具链
2. 格式检查（`cargo fmt --check`）
3. Workspace 测试（`cargo test --locked --workspace`）
4. Workspace 检查（`cargo check --locked --workspace`）
5. 安装 NSIS
6. 构建 release 安装包（标准版）
7. 安装、覆盖升级和卸载验证

可使用脚本检查 CI 状态：

```powershell
./scripts/_check-ci.ps1
```

> **重要**：必须等待 CI 成功后再创建标签。`release.yml` 会检查该提交是否有成功的 CI 运行记录。

## 步骤七：创建并推送标签

创建注释标签（annotated tag）：

```powershell
git tag -a vX.Y.Z -m "Veya vX.Y.Z"
git push origin vX.Y.Z
```

> **必须使用注释标签**（`-a`）。`release.yml` 会验证标签类型为 `tag`，不接受轻量标签。

推送标签后，`release.yml` 工作流自动触发。

## 步骤八：Release 工作流验证内容

`release.yml` 自动执行以下步骤：

1. **检出**：fetch-tags 拉取标签，恢复注释标签 ref
2. **安装工具链**：Rust 1.99.0 MSVC + rustfmt
3. **验证门禁**（`validate-release.ps1`）：
   - 标签格式 `vX.Y.Z`
   - Cargo 版本与标签匹配
   - 发布说明文件存在（`docs/releases/vX.Y.Z.md`）
   - 标签为注释标签
   - 标签提交为当前 HEAD
   - 标签提交在默认分支上
   - 该提交有成功的 CI 运行记录
4. **安装 NSIS**
5. **构建标准版安装包**（`build-installer.ps1`）
6. **安装 nightly 工具链**（用于 Win7 目标）
7. **构建 Windows 7 SP1 安装包**（`build-installer.ps1 -Win7`，使用 `x86_64-win7-windows-msvc` + build-std + 静态 CRT）
8. **生成发布报告**（`generate-release-report.ps1`，计算 SHA-256）
9. **发布 GitHub Release**（`softprops/action-gh-release@v2`），上传：
   - `veya-setup.exe` + `.sha256`
   - `veya-win7-setup.exe` + `.sha256`

## 步骤九：发布后验证

1. 在 GitHub Releases 页面确认发布成功
2. 下载 `veya-setup.exe`，独立计算 SHA-256 核对：

```powershell
(Get-FileHash -Algorithm SHA256 .\veya-setup.exe).Hash
```

3. 确认发布页正文包含构建信息与下载链接
4. 在安装机上验证安装、升级和卸载

## 失败重试

### 标签已推送但 Release 工作流失败

如果标签已推送但 Release 工作流在创建 GitHub Release 之前失败：

1. 修复问题并提交到 `main`
2. 手动运行 Release 工作流，传入原有标签：

```powershell
gh workflow run release.yml --repo NGLSL/Veya --ref main -f tag=vX.Y.Z
```

3. 重试仍验证该注释标签的版本、默认分支包含关系和 CI 成功记录
4. **不要移动已发布的标签**

### 其他情况

- 如果是功能代码问题，修复后重新提交，等待 CI 成功，再创建新标签
- 如果是 CI 配置问题，修复 workflow 文件后重试

## 注意事项

- 每次发布使用独立的注释标签，不要复用或移动标签
- 版本号使用语义化版本：主版本.次版本.修订号
- 发布说明必须人工编写，不要自动生成
- 仓库通过 `rust-toolchain.toml` 固定 Rust 版本，CI 和 Release 使用同一版本
- Windows 7 安装包使用 nightly 工具链和 `-Zbuild-std` 构建，需要 `--allow-downgrade`
