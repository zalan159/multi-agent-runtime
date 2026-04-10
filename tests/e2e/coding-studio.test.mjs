import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import { ClaudeAgentWorkspace, createCodingStudioWorkspace } from '../../dist/index.js';
import { createScratchDir, runWorkspaceScenario } from './_shared.mjs';

test('coding studio e2e generates a usable PRD through the prd role', { timeout: 240_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-coding');
  const outputFile = path.join(cwd, '10-prd/group-mentions.md');
  const workspace = new ClaudeAgentWorkspace({
    spec: createCodingStudioWorkspace({
      id: `coding-e2e-${Date.now()}`,
      name: 'Coding E2E',
      cwd,
    }),
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
  });

  assert.match(dispatch.resultText, /PRD|group mentions|acceptance/i);
  assert.match(fileText, /^# /m);
  assert.match(fileText, /## Goal/i);
  assert.match(fileText, /## User Story/i);
  assert.match(fileText, /## Scope/i);
  assert.match(fileText, /(## Non-Goals|\*\*Out of Scope:\*\*|## Out of Scope)/i);
  assert.match(fileText, /## Acceptance Criteria/i);
  assert.match(fileText, /@/);
  assert.ok(fileText.split(/\s+/).length <= 320, 'Expected concise PRD output');
});
