import type {
  RoleSpec,
  WorkspaceSpec,
  WorkspaceTurnAssignment,
  WorkspaceTurnPlan,
  WorkspaceTurnRequest,
} from './types.js';

export function buildWorkspaceTurnPrompt(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): string {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );

  const roleLines = spec.roles.map(role => describeRole(role)).join('\n');
  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;

  const preferredRoleLine = request.preferRoleId
    ? `Bias toward @${request.preferRoleId} if it is a strong fit, but do not force it if another role is clearly better.`
    : 'Choose the best role or small set of roles based on the actual work required.';

  return [
    `You are @${coordinatorRoleId}, the coordinator for the workspace \"${spec.name}\".`,
    'A user just sent one workspace-level message. Treat it as visible to the whole team, but you are responsible for deciding who should claim and execute.',
    preferredRoleLine,
    `You may assign at most ${maxAssignments} role(s).`,
    `Fallback role if no specialist clearly fits: @${fallbackRoleId}.`,
    'Return strict JSON only. Do not wrap it in markdown fences. Do not add prose before or after the JSON.',
    'JSON schema:',
    JSON.stringify(
      {
        responseText:
          'A short public coordinator response telling the user who is taking this and what happens next.',
        rationale: 'One short sentence explaining the routing decision.',
        assignments: [
          {
            roleId: 'one valid workspace role id',
            summary: 'a short assignment summary',
            instruction:
              'a concrete role-specific instruction that can be executed directly without more clarification',
          },
        ],
      },
      null,
      2,
    ),
    'Available roles:',
    roleLines,
    'User workspace message:',
    request.message,
    'Rules:',
    '- Use only valid roleId values from the available roles. Return bare ids like `prd`, not `@prd`.',
    '- The instruction for each assignment must be concrete and executable.',
    '- Prefer specialists over the coordinator when specialist work is required.',
    '- If the request is ambiguous, turn it into the smallest useful next step rather than asking a vague follow-up.',
    '- Keep responseText concise and public-facing.',
  ].join('\n\n');
}

export function parseWorkspaceTurnPlan(
  rawText: string,
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): WorkspaceTurnPlan {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const parsed = extractJsonObject(rawText);
  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;
  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );

  const allowedRoleIds = new Set(spec.roles.map(role => role.id));
  const assignmentsInput = Array.isArray(parsed?.assignments) ? parsed.assignments : [];
  const assignments = assignmentsInput
    .flatMap(value => normalizeAssignment(value, request))
    .filter(assignment => allowedRoleIds.has(assignment.roleId))
    .slice(0, maxAssignments);

  const finalAssignments = assignments.length > 0
    ? assignments
    : [buildFallbackAssignment(guessFallbackRoleId(spec, request.message, fallbackRoleId), request.message)];

  const responseText =
    typeof parsed?.responseText === 'string' && parsed.responseText.trim().length > 0
      ? parsed.responseText.trim()
      : buildFallbackResponseText(finalAssignments, spec.roles);

  return {
    coordinatorRoleId,
    responseText,
    assignments: finalAssignments,
    ...(typeof parsed?.rationale === 'string' && parsed.rationale.trim().length > 0
      ? { rationale: parsed.rationale.trim() }
      : {}),
  };
}

export function planWorkspaceTurnHeuristically(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
): WorkspaceTurnPlan {
  const coordinatorRoleId = spec.coordinatorRoleId ?? spec.defaultRoleId ?? spec.roles[0]?.id;
  if (!coordinatorRoleId) {
    throw new Error('Workspace has no coordinator role.');
  }

  const fallbackRoleId =
    spec.claimPolicy?.fallbackRoleId ?? spec.defaultRoleId ?? coordinatorRoleId;
  const maxAssignments = Math.max(
    1,
    request.maxAssignments ?? spec.claimPolicy?.maxAssignees ?? 1,
  );

  const assignments = request.preferRoleId
    ? [buildFallbackAssignment(request.preferRoleId, request.message)]
    : buildHeuristicAssignments(spec, request.message, fallbackRoleId, maxAssignments);

  return {
    coordinatorRoleId,
    responseText: buildFallbackResponseText(assignments, spec.roles),
    assignments: assignments.map(assignment => {
      const visibility =
        assignment.visibility ??
        request.visibility ??
        (spec.claimPolicy?.mode === 'claim' ? 'public' : undefined);
      return {
        ...assignment,
        ...(visibility ? { visibility } : {}),
      };
    }),
    rationale: 'Heuristic runtime routing selected claim candidates from role/message affinity.',
  };
}

function describeRole(role: RoleSpec): string {
  return [
    `- @${role.id} (${role.name})`,
    role.description ? `description: ${role.description}` : null,
    role.agent.description ? `agent: ${role.agent.description}` : null,
    role.outputRoot ? `output_root: ${role.outputRoot}` : null,
  ]
    .filter(Boolean)
    .join(' | ');
}

function extractJsonObject(rawText: string): Record<string, unknown> | null {
  const trimmed = rawText.trim();
  const fencedMatch = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/i);
  const candidate = fencedMatch?.[1] ?? trimmed;
  const direct = tryParseJson(candidate);
  if (direct && typeof direct === 'object' && !Array.isArray(direct)) {
    return direct as Record<string, unknown>;
  }

  const start = candidate.indexOf('{');
  const end = candidate.lastIndexOf('}');
  if (start >= 0 && end > start) {
    const sliced = candidate.slice(start, end + 1);
    const parsed = tryParseJson(sliced);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>;
    }
  }

  return null;
}

function tryParseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function normalizeAssignment(
  value: unknown,
  request: WorkspaceTurnRequest,
): WorkspaceTurnAssignment[] {
  if (!value || typeof value !== 'object') {
    return [];
  }

  const roleId = typeof (value as { roleId?: unknown }).roleId === 'string'
    ? (value as { roleId: string }).roleId.trim().replace(/^@+/, '')
    : '';
  const instruction = typeof (value as { instruction?: unknown }).instruction === 'string'
    ? (value as { instruction: string }).instruction.trim()
    : request.message.trim();

  if (!roleId || !instruction) {
    return [];
  }

  const assignment: WorkspaceTurnAssignment = {
    roleId,
    instruction,
  };

  const summary = typeof (value as { summary?: unknown }).summary === 'string'
    ? (value as { summary: string }).summary.trim()
    : '';
  if (summary) {
    assignment.summary = summary;
  }

  return [assignment];
}

function buildFallbackAssignment(
  roleId: string,
  message: string,
): WorkspaceTurnAssignment {
  return {
    roleId,
    summary: `Handle workspace request as @${roleId}`,
    instruction: message.trim(),
  };
}

function buildHeuristicAssignments(
  spec: WorkspaceSpec,
  message: string,
  fallbackRoleId: string,
  maxAssignments: number,
): WorkspaceTurnAssignment[] {
  const lowered = message.toLowerCase();
  const scored = spec.roles
    .map(role => ({ roleId: role.id, score: scoreRoleForMessage(role, lowered) }))
    .sort((left, right) => right.score - left.score || left.roleId.localeCompare(right.roleId));

  const selected = scored
    .filter(entry => entry.score > 0)
    .slice(0, maxAssignments)
    .map(entry => entry.roleId);

  const finalRoles = selected.length > 0 ? selected : [guessFallbackRoleId(spec, message, fallbackRoleId)];
  return Array.from(new Set(finalRoles)).map(roleId => buildFallbackAssignment(roleId, message));
}

function guessFallbackRoleId(
  spec: WorkspaceSpec,
  message: string,
  fallbackRoleId: string,
): string {
  const lowered = message.toLowerCase();
  let bestRoleId = fallbackRoleId;
  let bestScore = 0;

  for (const role of spec.roles) {
    const score = scoreRoleForMessage(role, lowered);
    if (score > bestScore) {
      bestRoleId = role.id;
      bestScore = score;
    }
  }

  return bestRoleId;
}

function scoreRoleForMessage(role: RoleSpec, loweredMessage: string): number {
  const corpus = [
    role.id,
    role.name,
    role.description,
    role.agent.description,
    role.outputRoot,
    ...(ROLE_HINTS[role.id] ?? []),
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase();

  let score = 0;
  for (const token of buildSearchTokens(corpus)) {
    if (token.length < 3) {
      continue;
    }
    if (loweredMessage.includes(token)) {
      score += token.length > 8 ? 3 : 1;
    }
  }

  return score;
}

function buildSearchTokens(text: string): string[] {
  return Array.from(
    new Set(
      text
        .split(/[^a-z0-9@-]+/i)
        .map(value => value.trim().toLowerCase())
        .filter(Boolean),
    ),
  );
}

const ROLE_HINTS: Record<string, string[]> = {
  pm: ['plan', 'milestone', 'scope', 'coordination'],
  prd: ['prd', 'requirement', 'requirements', 'spec', 'user story', 'acceptance criteria'],
  architect: ['architecture', 'design', 'interface', 'data model', 'technical plan'],
  coder: ['implement', 'implementation', 'code', 'patch', 'bug fix'],
  tester: ['test', 'qa', 'regression', 'verification'],
  reviewer: ['review', 'audit', 'bug finding'],
  ceo: ['priority', 'decision', 'approval', 'strategy'],
  finance: ['finance', 'monthly close', 'cash', 'invoice', 'invoices', 'subscription', 'revenue', 'kpi', 'burn', 'runway', 'budget'],
  tax: ['tax', 'filing', 'sales tax', 'vat', 'estimated tax'],
  admin: ['admin', 'vendor', 'operations', 'sop', 'checklist'],
  recruiter: ['recruit', 'candidate', 'hiring', 'interview'],
  lead: ['research lead', 'hypothesis', 'question framing'],
  scout: ['research', 'sources', 'web', 'compare', 'brief'],
  experimenter: ['experiment', 'metric', 'measure', 'test design'],
  critic: ['critique', 'risk', 'skeptic', 'assumption'],
  shangshu: ['coordination', 'governance', 'routing'],
  zhongshu: ['brief', 'mandate', 'task order', 'draft'],
  menxia: ['review', 'challenge', 'risk', 'red team'],
  gongbu: ['implementation', 'build', 'execution', 'tooling'],
  hubu: ['budget', 'resources', 'capacity', 'allocation'],
  libu: ['documentation', 'communication', 'release notes'],
  xingbu: ['compliance', 'safety', 'policy'],
  bingbu: ['release', 'incident', 'operations', 'rollout'],
};

function buildFallbackResponseText(
  assignments: WorkspaceTurnAssignment[],
  roles: RoleSpec[],
): string {
  const roleNames = assignments.map(assignment => {
    const role = roles.find(value => value.id === assignment.roleId);
    return role ? `@${role.id}` : assignment.roleId;
  });

  if (roleNames.length === 1) {
    return `${roleNames[0]} will take this next.`;
  }

  return `${roleNames.join(' and ')} will split this work.`;
}
