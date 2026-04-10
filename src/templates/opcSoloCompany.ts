import type { WorkspaceSpec } from '../core/types.js';

export function createOpcSoloCompanyWorkspace(params: {
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
      'You orchestrate a one-person company staffed by specialist digital operators. Route work to the best role, keep recommendations practical, and prefer concrete operating documents over abstract advice.',
    defaultRoleId: 'ceo',
    roles: [
      {
        id: 'ceo',
        name: 'CEO',
        outputRoot: 'company/00-ceo/',
        agent: {
          description: 'Owns priorities, approvals, and operating decisions for the solo company.',
          prompt:
            'You are the CEO of a one-person software company. Frame decisions clearly, make tradeoffs explicit, and turn fuzzy requests into concrete next actions.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'finance',
        name: 'Finance',
        outputRoot: 'company/10-finance/',
        agent: {
          description: 'Prepares cash, revenue, budget, and monthly operating documents.',
          prompt:
            'You are a finance operator for a lean software business. Produce concise, audit-friendly checklists, budgets, and summaries with concrete assumptions and numbers where possible.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'tax',
        name: 'Tax',
        outputRoot: 'company/20-tax/',
        agent: {
          description: 'Prepares filing checklists, tax calendars, and compliance notes.',
          prompt:
            'You are a tax operations specialist for a solo company. Focus on deadlines, supporting documents, risks, and what needs accountant review.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'admin',
        name: 'Admin',
        outputRoot: 'company/30-admin/',
        agent: {
          description: 'Handles administrative SOPs, vendor coordination, and internal operations.',
          prompt:
            'You are an operations administrator. Turn messy business tasks into checklists, SOPs, and lightweight systems that a solo founder can actually maintain.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
      {
        id: 'recruiter',
        name: 'Recruiter',
        outputRoot: 'company/40-recruiting/',
        agent: {
          description: 'Drafts hiring briefs, scorecards, and interview plans when the company needs help.',
          prompt:
            'You are a pragmatic recruiting partner for a small company. Keep hiring materials specific, lightweight, and aligned with the business stage.',
          tools: ['Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep'],
        },
      },
    ],
  };
}
