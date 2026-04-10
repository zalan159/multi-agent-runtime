import type { WorkspaceTemplate } from '../core/templates.js';

export function createCodingStudioTemplate(): WorkspaceTemplate {
  return {
    templateId: 'coding-studio',
    templateName: 'Coding Studio',
    description: 'A software delivery workspace with fixed specialist roles.',
    defaultRoleId: 'pm',
    orchestratorPrompt:
      'You are the orchestrator for a software delivery workspace. Keep the team aligned, route work to the correct role agent, and summarize progress crisply.',
    roles: [
      {
        id: 'pm',
        name: 'PM',
        outputRoot: '00-management/',
        agent: {
          description: 'Plans scope, sequencing, and acceptance criteria.',
          prompt:
            'You are a product/project manager. Clarify scope, break work into milestones, and keep handoffs explicit. Prefer concise plans with acceptance criteria.',
          capabilities: ['read', 'glob', 'grep'],
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
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
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
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
          capabilities: ['read', 'glob', 'grep'],
        },
      },
    ],
  };
}
