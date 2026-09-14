import { readFileSync } from 'node:fs';

const workflowPath = '.github/workflows/release.yml';
const workflow = readFileSync(workflowPath, 'utf8');
const failures = [];

function extractStepBlock(stepName) {
  const stepStart = workflow.indexOf(`      - name: ${stepName}`);
  if (stepStart === -1) {
    failures.push(`Missing workflow step: ${stepName}`);
    return '';
  }

  const nextStepStart = workflow.indexOf('\n      - name:', stepStart + `      - name: ${stepName}`.length);
  return workflow.slice(stepStart, nextStepStart === -1 ? workflow.length : nextStepStart);
}

function latestSemverTag(tags) {
  const parsed = tags
    .map((tag) => {
      const match = /^v(\d+)\.(\d+)\.(\d+)$/.exec(tag);
      if (!match) return null;
      return {
        tag,
        major: Number(match[1]),
        minor: Number(match[2]),
        patch: Number(match[3]),
      };
    })
    .filter(Boolean)
    .sort((a, b) =>
      b.major - a.major || b.minor - a.minor || b.patch - a.patch,
    );

  return parsed[0]?.tag ?? '';
}

function bumpVersion(current, bumpType) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(current);
  if (!match) {
    throw new Error(`Invalid semver: ${current}`);
  }

  let major = Number(match[1]);
  let minor = Number(match[2]);
  let patch = Number(match[3]);

  switch (bumpType) {
    case 'major':
      major += 1;
      minor = 0;
      patch = 0;
      break;
    case 'minor':
      minor += 1;
      patch = 0;
      break;
    case 'patch':
      patch += 1;
      break;
    default:
      throw new Error(`Invalid bump type: ${bumpType}`);
  }

  return `${major}.${minor}.${patch}`;
}

const latestTag = latestSemverTag(['v1.5.6', 'v1.5.5', 'v1.5.4', 'not-a-version']);
if (latestTag !== 'v1.5.6') {
  failures.push(`latestSemverTag should choose v1.5.6, got ${latestTag || '<empty>'}`);
}

const nextPatch = bumpVersion(latestTag.replace(/^v/, ''), 'patch');
if (nextPatch !== '1.5.7') {
  failures.push(`patch bump from latest tag v1.5.6 should be 1.5.7, got ${nextPatch}`);
}

const bumpJobStart = workflow.indexOf('  bump-version:');
const createReleaseStart = workflow.indexOf('\n  create-release:', bumpJobStart);
const bumpJob = workflow.slice(bumpJobStart, createReleaseStart === -1 ? workflow.length : createReleaseStart);
const bumpStep = extractStepBlock('Prepare release or open release PR');

const requiredSnippets = [
  {
    text: 'git tag --merged origin/main',
    message: 'bump-version must discover latest semver tag merged into origin/main',
  },
  {
    text: 'fetch-depth: 0',
    message: 'bump-version checkout must fetch full history so merged-tag detection is reliable',
  },
  {
    text: 'CURRENT="${BASE_TAG#v}"',
    message: 'bump-version must use latest tag as CURRENT when a semver tag exists',
  },
  {
    text: 'BRANCH="release/${TAG}"',
    message: 'bump-version must create a dedicated release branch',
  },
  {
    text: 'gh pr create',
    message: 'bump-version must open a PR instead of pushing directly to main',
  },
  {
    text: 'git push origin "${BRANCH}"',
    message: 'bump-version must push the release branch',
  },
  {
    text: 'SOURCE_VERSION=$(jq -r .version package.json)',
    message: 'bump-version must detect already-versioned main before opening a PR',
  },
  {
    text: 'git push origin "${TAG}"',
    message: 'bump-version must tag an already-versioned main commit after the bump PR is merged',
  },
  {
    text: 'echo "release-tag=${TAG}" >> "$GITHUB_OUTPUT"',
    message: 'bump-version must only release when it emits release-tag output',
  },
];

for (const snippet of requiredSnippets) {
  const haystack = snippet.text === 'fetch-depth: 0' ? bumpJob : bumpStep;
  if (!haystack.includes(snippet.text)) {
    failures.push(snippet.message);
  }
}

const forbiddenSnippets = [
  {
    text: 'git push origin HEAD:main',
    message: 'bump-version must not push directly to protected main',
  },
  {
    text: 'git tag "$TAG"',
    message: 'bump-version must use braced tag variable inside the already-versioned guard',
  },
  {
    text: 'CURRENT=$(jq -r .version package.json)',
    message: 'bump-version must not use stale package.json as the primary bump base',
  },
  {
    text: 'new-tag=${TAG}',
    message: 'bump-version must not emit new-tag after opening a bump PR',
  },
];

for (const snippet of forbiddenSnippets) {
  if (bumpStep.includes(snippet.text)) {
    failures.push(snippet.message);
  }
}

if (failures.length > 0) {
  console.error('release.yml bump PR flow check failed:');
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log('release.yml bump flow uses latest semver tag and creates a release PR.');
