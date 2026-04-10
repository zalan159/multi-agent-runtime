import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';

import { ClaudeAgentWorkspace, createOpcSoloCompanyWorkspace } from '../../dist/index.js';
import { createScratchDir, runWorkspaceScenario } from './_shared.mjs';

test('opc e2e generates a finance monthly close checklist through the finance role', { timeout: 240_000 }, async () => {
  const cwd = await createScratchDir('cteno-e2e-opc');
  const outputFile = path.join(cwd, 'company/10-finance/monthly-close-checklist.md');
  const workspace = new ClaudeAgentWorkspace({
    spec: createOpcSoloCompanyWorkspace({
      id: `opc-e2e-${Date.now()}`,
      name: 'OPC E2E',
      cwd,
    }),
  });

  const { dispatch, fileText } = await runWorkspaceScenario({
    workspace,
    task: {
      roleId: 'finance',
      summary: 'Prepare a monthly close checklist',
      instruction:
        'Create a markdown checklist at company/10-finance/monthly-close-checklist.md for a solo SaaS founder closing the month. Include cash review, invoices, subscriptions, payroll/contractors, tax prep handoff, and KPI review. Keep it practical and concise.',
    },
    expectedRoleId: 'finance',
    outputFile,
  });

  assert.match(dispatch.resultText, /finance|checklist|monthly close/i);
  assert.match(fileText, /## .*Cash Review/i);
  assert.match(fileText, /## .*(Invoices|Invoices & Receivables)/i);
  assert.match(fileText, /## .*(Subscriptions|Subscriptions & Revenue|Subscription Revenue & Metrics)/i);
  assert.match(fileText, /## .*Payroll/i);
  assert.match(fileText, /## .*Tax Prep Handoff/i);
  assert.match(fileText, /## .*KPI Review/i);
  const checkboxCount = (fileText.match(/- \[ \]/g) ?? []).length;
  assert.ok(checkboxCount >= 12, 'Expected a practical checklist with multiple actionable items');
});
