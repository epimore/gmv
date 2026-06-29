# GMV Agent Guide

See `.rust-skills/AGENTS.md` for Rust development guidelines.

## Repository Map

- `gmv`: `/home/ubuntu20/code/rs/mv/github/epimore/gmv`
- `gmv_pjsip`: `/home/ubuntu20/code/rs/mv/github/epimore/gmv_pjsip`
- `pigs`: `/home/ubuntu20/code/rs/mv/github/epimore/pigs`

`gmv/session` uses path dependencies to the sibling `gmv_pjsip` repository and
`pigs/base_db`.

## Common Commands

Non-interactive WSL sessions do not currently place Cargo in `PATH`; use the
explicit binary:

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv_pjsip
~/.cargo/bin/cargo test --all-features

cd /home/ubuntu20/code/rs/mv/github/epimore/gmv/session
~/.cargo/bin/cargo test
~/.cargo/bin/cargo check

cd /home/ubuntu20/code/rs/mv/github/epimore/gmv
~/.cargo/bin/cargo check --workspace
```

PJSIP is built from `third_party/pjproject-2.17.tar.gz` through
`session/build_pjsip_bootstrap.sh`. Do not commit generated `dist/`,
`config.log`, or other machine-local build outputs.

## Guard Service

- Product and architecture outline:
  `docs/product/guard-service-outline.md`
- Implementation plan:
  `docs/superpowers/plans/2026-06-23-guard-control-plane-implementation-plan.md`
- Architecture decisions:
  `docs/decisions/0002-guard-control-plane-boundaries.md`,
  `docs/decisions/0003-gmv-grpc-and-bus-protocol.md`,
  `docs/decisions/0004-guard-dual-database-storage.md`
- Acceptance and security baselines:
  `docs/quality/guard-control-plane-acceptance.md`,
  `docs/quality/guard-security-and-protocol-compatibility.md`
- Operations runbook: `docs/runbooks/guard-operations.md`

Do not modify `pigs`, `shared/protocol`, guard, session, stream, or avai code
until the implementation plan has been reviewed and explicitly approved. Do
not modify session, stream, or avai before the Guard independent-acceptance
gate in the plan has passed.

## Codex Desktop + WSL2

Codex Desktop 运行在 Windows、仓库位于 WSL2 时，不要把 UNC 仓库路径直接作为 Windows 命令工作目录；
该方式可能导致补丁或文件访问长时间挂起。固定从 Windows 本地目录启动 WSL，再在 Linux 内切换仓库：

```powershell
Set-Location C:/
wsl -e bash -lc "cd /home/ubuntu20/code/rs/mv/github/epimore/gmv && <command>"
```

- 已确认当前 Windows Codex 沙箱助手 `codex-windows-sandbox-setup.exe` 可能报“找不到指定的模块”或
  `orchestrator_helper_launch_canceled / 1223`。出现后视为 Windows 沙箱不可用，不重复调用依赖该助手的
  Windows 图片、补丁或文件工具；仓库操作统一走 WSL，Windows 临时附件通过 `/mnt/c/...` 在 WSL 内读取。
- 非交互 WSL 使用 `~/.cargo/bin/cargo`。
- 若继承的 `rg` 指向不可访问的 Windows Codex 安装目录，直接改用 WSL 的 `git grep`、`grep` 或明确的
  Linux 可执行文件，不重复尝试同一路径。
- 格式化优先限定受影响 crate，例如
  `~/.cargo/bin/cargo fmt -p gmv-session -p gmv-stream`；长时间无输出时使用 `timeout` 限时并检查残留进程。
- 宿主机对 UNC 文件写入或补丁工具挂起时，在 WSL 内执行确定性的临时补丁脚本，完成后用
  `cargo fmt`、测试和 `git diff --check` 验证；不要把临时脚本或机器本地产物写入仓库。
