# Multi-Agent Runtime

A small, extraction-friendly runtime for running role-based multi-agent workspaces on top of the official Claude Agent SDK.

This package gives us one unified protocol for:
- defining role agents
- delegating work to a specific role
- observing task lifecycle as events
- validating generated artifacts in end-to-end tests

The current adapter is Claude-first, but the protocol is intentionally generic enough to support a future Cteno-native adapter.

## What It Does

`@cteno/multi-agent-runtime` treats a workspace as:
- one persistent orchestrator session
- multiple named role agents declared as Claude subagents
- explicit role dispatches such as `prd`, `finance`, or `scout`
- a unified event stream for `workspace`, `dispatch`, `message`, `tool.progress`, and `result`

That maps well onto Claude's session-centric model while keeping our authoring model role-centric.

## Included Templates

### `Coding Studio`
A software delivery workspace.

Roles:
- `pm`
- `prd`
- `architect`
- `coder`
- `tester`
- `reviewer`

Typical outcome:
- PRDs
- implementation notes
- code changes
- test reports

### `OPC Solo Company`
A one-person company staffed by specialist digital operators.

Roles:
- `ceo`
- `finance`
- `tax`
- `admin`
- `recruiter`

Typical outcome:
- operating checklists
- finance docs
- tax prep handoff notes
- admin SOPs

### `Autoresearch`
A research-oriented workspace for scouting, synthesis, and lightweight experiment framing.

Roles:
- `lead`
- `scout`
- `experimenter`
- `critic`

Typical outcome:
- sourced briefs
- comparison notes
- experiment outlines
- research critiques

## Install

```bash
npm install @anthropic-ai/claude-agent-sdk
```

This package currently assumes:
- Node `>=20`
- a working Claude Code / Claude Agent SDK environment
- local Claude authentication already configured on the machine running the tests

## Quick Start

```ts
import {
  ClaudeAgentWorkspace,
  createCodingStudioWorkspace,
} from '@cteno/multi-agent-runtime';

const workspace = new ClaudeAgentWorkspace({
  spec: createCodingStudioWorkspace({
    id: 'demo-coding-studio',
    name: 'Demo Coding Studio',
    cwd: process.cwd(),
  }),
});

workspace.onEvent(event => {
  console.log(event.type, event);
});

await workspace.start();

const dispatch = await workspace.runRoleTask({
  roleId: 'prd',
  summary: 'Draft a PRD for group mentions',
  instruction:
    'Create a short markdown PRD at 10-prd/group-mentions.md for a group-chat mention feature. Include sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria.',
});

console.log(dispatch.status);
console.log(dispatch.resultText);
await workspace.close();
```

## Runtime API

### `assignRoleTask()`
Queues a task for a role and returns immediately with the local dispatch record.

### `runRoleTask()`
Queues a task and waits until:
- the dispatch reaches a terminal state
- Claude returns final result text for the delegated task, when available

This is the most convenient API for live e2e checks.

### `onEvent()`
Subscribes to the workspace event stream.

Useful event types:
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

## Development

```bash
npm install
npm run typecheck
npm run build
```

Helpful commands:

```bash
npm test
npm run clean
```

## Smoke Commands

These are useful for manual exploration, but they are not our release-quality validation layer.

```bash
npm run smoke:coding
npm run smoke:opc
npm run smoke:autoresearch
```

## Live E2E Tests

These are the important ones.

They make real Claude calls and assert:
- the workspace initializes correctly
- the dispatch is queued, started, completed, and produces a final result
- the delegated role matches the expected role
- the expected file is actually generated
- the generated content matches template-specific acceptance checks

Run them individually:

```bash
npm run e2e:coding
npm run e2e:opc
npm run e2e:autoresearch
```

Run the full suite sequentially:

```bash
npm run e2e
```

### Current E2E Coverage

#### Coding Studio
Checks that:
- the `prd` role is used
- `10-prd/group-mentions.md` is created
- the file contains `Goal`, `User Story`, `Scope`, `Non-Goals`, and `Acceptance Criteria`
- the output is concise and implementation-oriented

#### OPC Solo Company
Checks that:
- the `finance` role is used
- `company/10-finance/monthly-close-checklist.md` is created
- the file contains monthly close sections for cash, invoices, subscriptions, payroll, tax prep, and KPIs
- the output is a real checklist with multiple actionable items

#### Autoresearch
Checks that:
- the `scout` role is used
- the workspace emits multiple research progress events
- `research/10-scout/mention-patterns.md` is created
- the brief includes `Implications for Cteno`
- the brief references tools like Slack and GitHub
- the brief includes at least three source links

## Design Notes

### Why this shape?
Claude's public interface behaves more like:
- sessions
- subagents
- task lifecycle notifications

and less like a fully exposed graph runtime.

So this package keeps the runtime thin and practical:
- `WorkspaceSpec` defines the workspace and its roles
- `ClaudeAgentWorkspace` adapts those roles into Claude subagents
- dispatches are provider-neutral records in our protocol
- events are normalized before they reach callers

### Important current limitation
Role-task correlation is still FIFO:
- each `assignRoleTask()` call queues a local dispatch
- the next Claude `task_started` event is matched to the next queued dispatch

This is reliable when the orchestrator is the only source of subagent launches.
If we later allow autonomous, unrelated subagent launches in the same session, we should strengthen correlation beyond FIFO.

## Status

Current status for the Claude adapter:
- live dispatching works
- delegated role tasks produce normalized `dispatch.*` events
- final result text is attached back onto the dispatch as `resultText`
- all three built-in templates have passing live e2e coverage

That makes this package good enough to continue hardening toward an open-source split, with Cteno integration as the next adapter layer.

## Open Source Readiness

See:
- [CONTRIBUTING.md](/Users/zal/Cteno/packages/multi-agent-runtime/CONTRIBUTING.md)
- [OPEN_SOURCE_CHECKLIST.md](/Users/zal/Cteno/packages/multi-agent-runtime/OPEN_SOURCE_CHECKLIST.md)
