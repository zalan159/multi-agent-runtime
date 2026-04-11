import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import {
  CodexSdkWorkspace,
  createAutoresearchTemplate,
  createCodexWorkspaceProfile,
  instantiateWorkspace,
} from '../../dist/index.js';
import {
  countHttpUrls,
  countMarkdownLinks,
  createScratchDir,
  runWorkspaceTurnScenario,
} from './_shared.mjs';

test('codex sdk e2e routes an autoresearch workspace turn to scout and writes a sourced brief', { timeout: 420_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-codex-autoresearch');
  const outputFile = path.join(cwd, 'research/10-scout/mention-patterns.md');
  const workspace = new CodexSdkWorkspace({
    spec: instantiateWorkspace(
      createAutoresearchTemplate(),
      {
        id: `codex-autoresearch-e2e-${Date.now()}`,
        name: 'Codex Autoresearch E2E',
        cwd,
      },
      createCodexWorkspaceProfile({
        model: 'gpt-5.1-codex-mini',
      }),
    ),
    skipGitRepoCheck: true,
    approvalPolicy: 'never',
    sandboxMode: 'workspace-write',
    networkAccessEnabled: true,
    webSearchMode: 'live',
  });

  const { dispatch, turn, events, fileText } = await runWorkspaceTurnScenario({
    workspace,
    message:
      'Research how collaboration tools like Slack and GitHub handle @mentions and directed attention, then write a concise brief to research/10-scout/mention-patterns.md with three short source links and a final section called Implications for Cteno.',
    expectedRoleId: 'scout',
    outputFile,
    timeoutMs: 360_000,
    resultTimeoutMs: 30_000,
  });

  assert.match(turn.plan.responseText, /@scout|research/i);
  const progressEvents = events.filter(event => event.type === 'dispatch.progress' && event.dispatch.dispatchId === dispatch.dispatchId);
  assert.ok(progressEvents.length >= 2, 'Expected multiple progress updates during research');
  assert.match(dispatch.resultText, /research|source|Cteno/i);
  assert.match(fileText, /Implications for Cteno/i);
  assert.match(fileText, /Slack/i);
  assert.match(fileText, /GitHub/i);
  assert.ok(
    Math.max(countMarkdownLinks(fileText), countHttpUrls(fileText)) >= 3,
    'Expected at least three source links in the research brief',
  );
});
