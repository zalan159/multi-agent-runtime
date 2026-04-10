import type { WorkspaceSpec } from '../core/types.js';

export function createCodingStudioWorkspace(params: {
  id: string;
  name: string;
  cwd: string;
  model?: string;
}): WorkspaceSpec {
  return {
    id: params.id,
    name: params.name,
    provider: 'claude-agent-sdk',
    model: params.model ?? 'claude-sonnet-4-5',
    cwd: params.cwd,
    permissionMode: 'acceptEdits',
    settingSources: ['project'],
    allowedTools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'Bash'],
    orchestratorPrompt:
      'You are the orchestrator for a software delivery workspace. Keep the team aligned, route work to the correct role agent, and summarize progress crisply.',
    defaultRoleId: 'pm',
    roles: [
      {
        id: 'pm',
        name: 'PM',
        outputRoot: '00-management/',
        agent: {
          description: 'Plans scope, sequencing, and acceptance criteria.',
          prompt:
            'You are a product/project manager. Clarify scope, break work into milestones, and keep handoffs explicit. Prefer concise plans with acceptance criteria.',
          tools: ['Read', 'Glob', 'Grep'],
        },
      },
      {
        id: 'prd',
        name: 'PRD',
        outputRoot: '10-prd/',
        agent: {
          description: 'Writes product requirement docs and task definitions.',
          prompt:
            'You write implementation-ready PRDs. Be concrete about user stories, scope, edge cases, and acceptance criteria.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'architect',
        name: 'Architect',
        outputRoot: '30-arch/',
        agent: {
          description: 'Designs implementation plans and system changes.',
          prompt:
            'You are a software architect. Produce pragmatic design notes, data flow decisions, interfaces, and risks before coding starts.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'coder',
        name: 'Coder',
        outputRoot: '40-code/',
        agent: {
          description: 'Implements code changes and keeps diffs focused.',
          prompt:
            'You are an implementation specialist. Make the requested change with minimal churn, explain assumptions briefly, and keep code consistent with the repository style.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'Bash'],
          permissionMode: 'acceptEdits',
        },
      },
      {
        id: 'tester',
        name: 'Tester',
        outputRoot: '50-test/',
        agent: {
          description: 'Runs tests, validates behavior, and reports regressions.',
          prompt:
            'You are a verification specialist. Run the narrowest useful checks first, surface failures clearly, and report residual risks if full coverage is not possible.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'Bash'],
        },
      },
      {
        id: 'reviewer',
        name: 'Reviewer',
        outputRoot: '60-review/',
        agent: {
          description: 'Reviews changes for bugs, regressions, and missing tests.',
          prompt:
            'You perform code review with a bug-finding mindset. Prioritize correctness, regressions, and missing validation over style commentary.',
          tools: ['Read', 'Glob', 'Grep'],
        },
      },
    ],
  };
}
