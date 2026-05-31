import { readFileSync } from 'node:fs';

const workflowPath = '.github/workflows/release.yml';
const workflow = readFileSync(workflowPath, 'utf8');

const jobNames = ['update-homebrew', 'update-winget'];
const failures = [];

function normalizeCondition(condition) {
  return condition.replace(/\s+/g, ' ').trim();
}

function extractJobIfBlock(jobName) {
  const jobStart = workflow.indexOf(`  ${jobName}:`);
  if (jobStart === -1) {
    failures.push(`Missing ${jobName} job`);
    return '';
  }

  const nextJobMatch = workflow
    .slice(jobStart + `  ${jobName}:`.length)
    .match(/\n  [A-Za-z0-9_-]+:\n/g);
  const nextJobStart = nextJobMatch
    ? workflow.indexOf(nextJobMatch[0], jobStart + `  ${jobName}:`.length)
    : -1;
  const jobBlock = workflow.slice(jobStart, nextJobStart === -1 ? workflow.length : nextJobStart);
  const ifMatch = jobBlock.match(/\n    if: \|\n([\s\S]*?)(?=\n    [a-zA-Z_-]+:|\n\n|$)/);
  if (!ifMatch) {
    failures.push(`Missing multiline if condition on ${jobName}`);
    return '';
  }

  return ifMatch[1]
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .join(' ');
}

function registryJobShouldRun({ tagName, updateRegistries, forceTagOverrideRegistryUpdate, isBackbuild, publishUpdaterSucceeded }) {
  return (
    publishUpdaterSucceeded &&
    (!isBackbuild || forceTagOverrideRegistryUpdate === true) &&
    (tagName.endsWith('.0') || updateRegistries === true || forceTagOverrideRegistryUpdate === true)
  );
}

function conditionMatchesExpectedGuard(condition) {
  const expected = [
    'always() &&',
    "needs.publish-updater.result == 'success' &&",
    "(needs.create-release.outputs.is-backbuild != 'true' || inputs.force_tag_override_registry_update) &&",
    "(endsWith(needs.create-release.outputs.tag-name, '.0') || inputs.update_registries || inputs.force_tag_override_registry_update)",
  ].join(' ');

  return condition === expected;
}

if (!workflow.includes('force_tag_override_registry_update:')) {
  failures.push('workflow_dispatch must expose force_tag_override_registry_update checkbox for explicit tag_override registry updates');
}

for (const jobName of jobNames) {
  const condition = normalizeCondition(extractJobIfBlock(jobName));
  if (!condition) continue;

  if (!conditionMatchesExpectedGuard(condition)) {
    failures.push(`${jobName} registry condition must exactly match the expected backbuild-force guard`);
  }

  if (!condition.includes("endsWith(needs.create-release.outputs.tag-name, '.0')")) {
    failures.push(`${jobName} must auto-run for major/minor tags ending in .0`);
  }

  if (!condition.includes("needs.create-release.outputs.is-backbuild != 'true'") || !condition.includes('inputs.force_tag_override_registry_update')) {
    failures.push(`${jobName} must skip tag_override backbuilds unless force_tag_override_registry_update is checked`);
  }

  if (!condition.includes('inputs.update_registries')) {
    failures.push(`${jobName} must allow manual registry updates for patch releases`);
  }

  if (!condition.includes('inputs.force_tag_override_registry_update')) {
    failures.push(`${jobName} must allow explicit registry updates for tag_override runs`);
  }

  if (condition.includes("inputs.update_registries == 'true'")) {
    failures.push(
      `${jobName} compares boolean workflow_dispatch input update_registries to string 'true'; ` +
        'use boolean truthiness so checked patch-release override enables registry jobs',
    );
  }
}

const scenarios = [
  {
    name: 'minor .0 release auto-runs registry updates without manual override',
    input: { tagName: 'v1.6.0', updateRegistries: false, forceTagOverrideRegistryUpdate: false, isBackbuild: false, publishUpdaterSucceeded: true },
    expected: true,
  },
  {
    name: 'patch release skips registry updates by default',
    input: { tagName: 'v1.5.7', updateRegistries: false, forceTagOverrideRegistryUpdate: false, isBackbuild: false, publishUpdaterSucceeded: true },
    expected: false,
  },
  {
    name: 'patch release runs registry updates when update_registries is checked',
    input: { tagName: 'v1.5.7', updateRegistries: true, forceTagOverrideRegistryUpdate: false, isBackbuild: false, publishUpdaterSucceeded: true },
    expected: true,
  },
  {
    name: 'tag_override backbuild skips registry updates even when update_registries is checked',
    input: { tagName: 'v1.5.6', updateRegistries: true, forceTagOverrideRegistryUpdate: false, isBackbuild: true, publishUpdaterSucceeded: true },
    expected: false,
  },
  {
    name: 'tag_override backbuild runs registry updates when force checkbox is checked',
    input: { tagName: 'v1.5.6', updateRegistries: false, forceTagOverrideRegistryUpdate: true, isBackbuild: true, publishUpdaterSucceeded: true },
    expected: true,
  },
];

for (const scenario of scenarios) {
  const actual = registryJobShouldRun(scenario.input);
  if (actual !== scenario.expected) {
    failures.push(
      `${scenario.name}: expected ${scenario.expected ? 'run' : 'skip'}, got ${actual ? 'run' : 'skip'}`,
    );
  }
}

if (failures.length > 0) {
  console.error('release.yml registry condition check failed:');
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log('release.yml registry conditions allow .0 auto-updates, patch override, and explicit tag_override force.');
