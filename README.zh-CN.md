# Multi-Agent Runtime

[English](./README.md) | [简体中文](./README.zh-CN.md)

一个面向多 Agent 协作场景的轻量运行时，目前已经对接官方 Claude Agent SDK 和 Codex SDK。

这套包的目标不是再造一个通用图编排框架，而是提供一层统一协议，用来完成这些事：
- 定义角色型 Agent
- 把任务明确委派给某个角色
- 以统一事件流观察任务生命周期
- 用真实 Claude 调用做端到端验收

当前仓库已经同时包含：
- TypeScript 参考实现
- 面向 Cteno 嵌入场景的 Rust 协议与运行时实现

当前已经有两套 adapter：
- `ClaudeAgentWorkspace`：对接 Anthropic Claude Agent SDK
- `CodexSdkWorkspace`：对接 OpenAI Codex SDK

协议本身仍然是为后续接入 Cteno 原生 adapter 预留的。

## 它解决什么问题

`@cteno/multi-agent-runtime` 把一个多 Agent 工作间建模成：
- 一个持久的 orchestrator session
- 多个具名角色 Agent，作为 Claude subagents 注册
- 显式的角色派发，比如 `prd`、`finance`、`scout`
- 一条统一的事件流，覆盖 `workspace`、`dispatch`、`message`、`tool.progress`、`result`

这和 Claude 当前公开接口的形态比较契合：Claude 更像 `session + subagent + task lifecycle`，而不是一套直接暴露给用户的图编排运行时。

## 内置模板

### `Coding Studio`
一个面向软件交付的工作间。

角色：
- `pm`
- `prd`
- `architect`
- `coder`
- `tester`
- `reviewer`

典型交付物：
- PRD
- 设计说明
- 代码修改
- 测试结果

### `OPC Solo Company`
一个“一人公司数字员工”工作间。

角色：
- `ceo`
- `finance`
- `tax`
- `admin`
- `recruiter`

典型交付物：
- 经营清单
- 财务文档
- 税务准备材料
- 行政 SOP

### `Autoresearch`
一个面向研究与资料整理的工作间。

角色：
- `lead`
- `scout`
- `experimenter`
- `critic`

典型交付物：
- 带来源的研究简报
- 对比分析
- 实验设计草案
- 研究质疑与补充

## 安装

```bash
npm install @anthropic-ai/claude-agent-sdk @openai/codex-sdk
```

当前假设：
- Node `>=20`
- 本机已经可以正常使用 Claude Code / Claude Agent SDK
- 本机已经可以正常使用 Codex CLI / Codex SDK
- 本地已经完成 Claude 认证
- 本地已经完成 Codex 认证

## 快速开始

```ts
import {
  ClaudeAgentWorkspace,
  createClaudeWorkspaceProfile,
  createCodingStudioTemplate,
  instantiateWorkspace,
} from '@cteno/multi-agent-runtime';

const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createCodingStudioTemplate(),
    {
      id: 'demo-coding-studio',
      name: 'Demo Coding Studio',
      cwd: process.cwd(),
    },
    createClaudeWorkspaceProfile(),
  ),
});

workspace.onEvent(event => {
  console.log(event.type, event);
});

await workspace.start();

const dispatch = await workspace.runRoleTask({
  roleId: 'prd',
  summary: '起草群聊 @mention 的 PRD',
  instruction:
    'Create a short markdown PRD at 10-prd/group-mentions.md for a group-chat mention feature. Include sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria.',
});

console.log(dispatch.status);
console.log(dispatch.resultText);
await workspace.close();
```

### Codex SDK 示例

```ts
import {
  CodexSdkWorkspace,
  createCodingStudioTemplate,
  createCodexWorkspaceProfile,
  instantiateWorkspace,
} from '@cteno/multi-agent-runtime';

const workspace = new CodexSdkWorkspace({
  spec: instantiateWorkspace(
    createCodingStudioTemplate(),
    {
      id: 'demo-codex-coding-studio',
      name: 'Demo Codex Coding Studio',
      cwd: process.cwd(),
    },
    createCodexWorkspaceProfile({
      model: 'gpt-5.1-codex-mini',
    }),
  ),
  skipGitRepoCheck: true,
  approvalPolicy: 'never',
  sandboxMode: 'workspace-write',
});

await workspace.start();
const dispatch = await workspace.runRoleTask({
  roleId: 'prd',
  summary: '起草群聊 @mention 的 PRD',
  instruction:
    'Create a short markdown PRD at 10-prd/group-mentions.md for a group-chat mention feature.',
});

console.log(dispatch.status);
console.log(dispatch.resultText);
await workspace.close();
```

## 运行时 API

### `assignRoleTask()`
把任务排队发给某个角色，立即返回本地 dispatch 记录。

### `runRoleTask()`
把任务发给某个角色，并等待：
- dispatch 进入终态
- Claude 返回该任务的最终文本结果（如果有）

这是最适合做 live e2e 的接口。

### `onEvent()`
订阅工作间事件流。

常用事件：
- `workspace.started`
- `workspace.initialized`
- `workspace.state.changed`
- `dispatch.queued`
- `dispatch.started`
- `dispatch.progress`
- `dispatch.completed`
- `dispatch.result`
- `tool.progress`
- `result`
- `error`

## 开发

```bash
npm install
npm run typecheck
npm run build
```

常用命令：

```bash
npm test
npm run clean
```

### Rust Workspace

```bash
cd rust
cargo test
```

当前 Rust crates：
- `multi-agent-protocol`
- `multi-agent-runtime-core`
- `multi-agent-runtime-cteno`

## Smoke 命令

这些命令适合人工观察行为，但不作为发布质量门槛。

```bash
npm run smoke:coding
npm run smoke:opc
npm run smoke:autoresearch
```

## Live E2E

真正重要的是这些测试。

它们会发起真实 Claude 调用，并断言：
- workspace 能正常初始化
- dispatch 经过 queued / started / completed / result
- 委派给了正确的角色
- 目标文件真的生成了
- 生成内容符合模板对应的验收标准

单独运行：

```bash
npm run e2e:coding
npm run e2e:codex
npm run e2e:opc
npm run e2e:autoresearch
```

顺序跑完整套：

```bash
npm run e2e
```

### 当前 E2E 覆盖

#### Coding Studio
检查：
- 使用的是 `prd` 角色
- 生成了 `10-prd/group-mentions.md`
- 文件包含 `Goal`、`User Story`、`Scope`、`Non-Goals`（或语义等价写法）、`Acceptance Criteria`

#### Codex Coding Studio
检查：
- `CodexSdkWorkspace` 能复用 role thread
- `prd` 角色会生成 `10-prd/group-mentions.md`
- 生成的 PRD 包含预期章节
- 输出足够简洁，适合继续交付

#### OPC Solo Company
检查：
- 使用的是 `finance` 角色
- 生成了 `company/10-finance/monthly-close-checklist.md`
- 文件包含现金、发票、订阅、薪资、税务、KPI 等关键章节
- 内容确实是可执行 checklist，不只是摘要

#### Autoresearch
检查：
- 使用的是 `scout` 角色
- 研究过程中出现多个 progress 事件
- 生成了 `research/10-scout/mention-patterns.md`
- 包含 `Implications for Cteno`
- 提到了 `Slack`、`GitHub`
- 至少包含 3 个来源链接

## 设计说明

### 为什么长这样
Claude 当前公开接口更偏向：
- session
- subagent
- task lifecycle notification

而不是直接暴露给用户一个通用 graph runtime。

所以这个包刻意做得比较薄：
- `WorkspaceSpec` 定义工作间和角色
- `ClaudeAgentWorkspace` 把这些角色映射成 Claude subagents
- dispatch 是我们自己的中立协议对象
- 外部调用方拿到的是统一后的事件流

### 当前已知限制
角色任务和 Claude task 的关联目前还是 FIFO：
- 每次 `assignRoleTask()` 会先在本地排一个 dispatch
- 下一个 Claude `task_started` 会被绑定到下一个排队中的 dispatch

当 orchestrator 是唯一的子 agent 启动源时，这个策略是稳定的。
如果以后同一个 session 中允许更多自治型并发 subagent 启动，就需要把这层关联做得更强。

## 当前状态

Claude adapter 目前已经做到：
- 真正可用的 live dispatch
- 统一的 `dispatch.*` 事件
- Claude 最终文本结果会回挂到 dispatch 的 `resultText`
- 三个内置模板都具备通过的 live e2e

Rust 侧目前已经做到：
- provider-neutral 协议类型
- 核心 dispatch 生命周期 runtime
- Cteno adapter 的 trait 与 bootstrap 流程
- `cargo test` 已通过

所以它已经足够进入开源孵化阶段，并可以作为后续接入 Cteno adapter 的基础。

## 相关文档

- [贡献指南](./CONTRIBUTING.md)
- [开源准备清单](./OPEN_SOURCE_CHECKLIST.md)
