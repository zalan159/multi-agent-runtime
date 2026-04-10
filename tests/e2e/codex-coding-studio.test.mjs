import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  CodexSdkWorkspace,
  createCodingStudioTemplate,
  createCodexWorkspaceProfile,
  instantiateWorkspace,
} from '../../dist/index.js';
import { createScratchDir, runWorkspaceScenario } from './_shared.mjs';

test('codex sdk e2e generates a usable PRD through a reusable role thread', { timeout: 240_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-codex-coding');
  const outputFile = path.join(cwd, '10-prd/group-mentions.md');
  const workspace = new CodexSdkWorkspace({
    spec: instantiateWorkspace(
      createCodingStudioTemplate(),
      {
        id: `codex-coding-e2e-${Date.now()}`,
        name: 'Codex Coding E2E',
        cwd,
      },
      createCodexWorkspaceProfile({
        model: 'gpt-5.1-codex-mini',
      }),
    ),
    skipGitRepoCheck: true,
    approvalPolicy: 'never',
    sandboxMode: 'workspace-write',
  });

  const { dispatch, fileText } = await runWorkspaceScenario({
    workspace,
    task: {
      roleId: 'prd',
      summary: 'Create a PRD stub for group mentions',
      instruction:
        'Create a short markdown PRD at 10-prd/group-mentions.md for a group-chat mention feature. Include sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria. Keep it under 250 words.',
    },
    expectedRoleId: 'prd',
    outputFile,
    timeoutMs: 180_000,
    resultTimeoutMs: 20_000,
  });

  assert.match(dispatch.resultText, /PRD|group mentions|acceptance/i);
  assert.match(fileText, /^# /m);
  assert.match(fileText, /## Goal/i);
  assert.match(fileText, /## User Story/i);
  assert.match(fileText, /## Scope/i);
  assert.match(fileText, /(## Non-Goals|\*\*Out of Scope:\*\*|## Out of Scope)/i);
  assert.match(fileText, /## Acceptance Criteria/i);
});
