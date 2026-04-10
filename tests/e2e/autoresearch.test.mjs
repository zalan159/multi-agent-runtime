import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  createAutoresearchTemplate,
  createClaudeWorkspaceProfile,
  instantiateWorkspace,
} from '../../dist/index.js';
import { countMarkdownLinks, createScratchDir, runWorkspaceScenario } from './_shared.mjs';

test('autoresearch e2e delegates to scout, uses research workflow, and writes a sourced brief', { timeout: 360_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-autoresearch');
  const outputFile = path.join(cwd, 'research/10-scout/mention-patterns.md');
  const workspace = new ClaudeAgentWorkspace({
    spec: instantiateWorkspace(
      createAutoresearchTemplate(),
      {
        id: `autoresearch-e2e-${Date.now()}`,
        name: 'Autoresearch E2E',
        cwd,
      },
      createClaudeWorkspaceProfile(),
    ),
  });

  const { dispatch, events, fileText } = await runWorkspaceScenario({
    workspace,
    task: {
      roleId: 'scout',
      summary: 'Research the product pattern behind @mentions',
      instruction:
        'Use web research if helpful, then create a concise markdown brief at research/10-scout/mention-patterns.md comparing how Slack, GitHub, or similar collaboration tools handle @mentions and directed attention. Include 3 short source links and a final section called Implications for Cteno.',
    },
    expectedRoleId: 'scout',
    outputFile,
    timeoutMs: 300_000,
    resultTimeoutMs: 30_000,
  });

  const progressEvents = events.filter(event => event.type === 'dispatch.progress' && event.dispatch.dispatchId === dispatch.dispatchId);
  assert.ok(progressEvents.length >= 2, 'Expected multiple progress updates during research');
  assert.match(dispatch.resultText, /research|source|Cteno/i);
  assert.match(fileText, /Implications for Cteno/i);
  assert.match(fileText, /Slack/i);
  assert.match(fileText, /GitHub/i);
  assert.ok(countMarkdownLinks(fileText) >= 3, 'Expected at least three source links in the research brief');
});
