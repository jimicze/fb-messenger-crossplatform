import { readFileSync } from 'node:fs';

const workflow = readFileSync('.github/workflows/release.yml', 'utf8');
const failures = [];

const requiredSnippets = [
  {
    text: '$generatedManifestDir = Join-Path $manifestDir "manifests\\j\\jimicze\\MessengerX\\$VERSION"',
    message: 'winget workflow must use wingetcreate output path under winget-manifests\\manifests',
  },
  {
    text: '$localePath = Join-Path $generatedManifestDir "jimicze.MessengerX.locale.en-US.yaml"',
    message: 'locale manifest path must be derived from generatedManifestDir',
  },
  {
    text: '.\\wingetcreate.exe submit $generatedManifestDir',
    message: 'winget submit must use the generated manifest directory',
  },
];

const forbiddenSnippets = [
  {
    text: '$manifestDir\j\jimicze\MessengerX\$VERSION',
    message: 'winget workflow must not use the pre-wingetcreate path missing the manifests segment',
  },
  {
    text: '.\\wingetcreate.exe submit "$manifestDir\j\jimicze\MessengerX\$VERSION"',
    message: 'winget submit must not use a path missing the manifests segment',
  },
];

for (const snippet of requiredSnippets) {
  if (!workflow.includes(snippet.text)) {
    failures.push(snippet.message);
  }
}

for (const snippet of forbiddenSnippets) {
  if (workflow.includes(snippet.text)) {
    failures.push(snippet.message);
  }
}

if (failures.length > 0) {
  console.error('release.yml winget manifest path check failed:');
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log('release.yml winget path uses wingetcreate generated manifests directory.');
