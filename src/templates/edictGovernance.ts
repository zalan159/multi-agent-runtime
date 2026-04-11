import type { WorkspaceTemplate } from '../core/templates.js';

export function createEdictGovernanceTemplate(): WorkspaceTemplate {
  return {
    templateId: 'edict-governance',
    templateName: 'Three Departments Six Ministries',
    description: 'A governance-style multi-agent workspace for coordinated planning, review, execution, and oversight.',
    defaultRoleId: 'shangshu',
    coordinatorRoleId: 'shangshu',
    orchestratorPrompt:
      'You coordinate a governance-style multi-agent workspace. Keep responsibilities crisp, route work to the right ministry, and enforce review before completion.',
    claimPolicy: {
      mode: 'claim',
      claimTimeoutMs: 1500,
      maxAssignees: 2,
      allowSupportingClaims: true,
      fallbackRoleId: 'shangshu',
    },
    activityPolicy: {
      publishUserMessages: true,
      publishCoordinatorMessages: true,
      publishDispatchLifecycle: true,
      publishMemberMessages: true,
      defaultVisibility: 'public',
    },
    roles: [
      {
        id: 'shangshu',
        name: 'Shangshu',
        outputRoot: 'governance/00-shangshu/',
        agent: {
          description: 'Coordinates ministries, sequence, and final closure.',
          prompt:
            'You are the chief coordinator. Route work, close loops, demand concrete outputs, and make sure every major task ends with a clear disposition.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'zhongshu',
        name: 'Zhongshu',
        outputRoot: 'governance/10-zhongshu/',
        agent: {
          description: 'Drafts mission briefs, task orders, and structured plans.',
          prompt:
            'You draft precise task orders, plans, and briefs. Convert vague goals into crisp instructions with deliverables and milestones.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'menxia',
        name: 'Menxia',
        outputRoot: 'governance/20-menxia/',
        agent: {
          description: 'Reviews proposals, challenges assumptions, and enforces red-team scrutiny.',
          prompt:
            'You are the review gate. Challenge weak reasoning, surface risks, and reject plans that are not yet executable or safe.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'gongbu',
        name: 'Gongbu',
        outputRoot: 'governance/30-gongbu/',
        agent: {
          description: 'Executes implementation, build-out, and tooling work.',
          prompt:
            'You are the implementation ministry. Build concrete outputs, keep execution disciplined, and report exact deliverables.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
        },
      },
      {
        id: 'hubu',
        name: 'Hubu',
        outputRoot: 'governance/40-hubu/',
        agent: {
          description: 'Tracks resources, budgets, dependencies, and allocation tradeoffs.',
          prompt:
            'You manage resources and constraints. Quantify budget, headcount, token, or time tradeoffs and keep plans grounded in capacity.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'libu',
        name: 'Libu',
        outputRoot: 'governance/50-libu/',
        agent: {
          description: 'Prepares communication, docs, release notes, and external-facing materials.',
          prompt:
            'You own communication and documentation. Package decisions and outputs into clear artifacts others can consume quickly.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'xingbu',
        name: 'Xingbu',
        outputRoot: 'governance/60-xingbu/',
        agent: {
          description: 'Owns compliance, safety, and rule enforcement.',
          prompt:
            'You enforce quality, compliance, and safety constraints. Flag violations early and insist on auditable fixes.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'bingbu',
        name: 'Bingbu',
        outputRoot: 'governance/70-bingbu/',
        agent: {
          description: 'Handles operations, release readiness, incident response, and escalation.',
          prompt:
            'You manage operational readiness. Focus on rollout, incident handling, fallback plans, and postmortem discipline.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
        },
      },
    ],
  };
}
