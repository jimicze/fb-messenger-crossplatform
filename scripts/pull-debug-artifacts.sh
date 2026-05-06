#!/usr/bin/env bash
# Downloads all debug artifacts from the latest completed Test Build CI run.
# Output goes to dist/debug/ (already gitignored via dist/).
#
# Usage:
#   npm run artifacts:pull          # latest completed run
#   npm run artifacts:pull -- 12345 # specific run ID
#
# Requires: gh CLI (https://cli.github.com) authenticated with repo access.

set -euo pipefail

WORKFLOW="Test Build"
OUT_DIR="dist/debug"

# Allow passing a specific run ID as first argument.
if [ "${1-}" != "" ]; then
    RUN_ID="$1"
    echo "Using run ID: ${RUN_ID}"
else
    echo "Looking for the latest completed '${WORKFLOW}' run..."
    RUN_ID=$(gh run list \
        --workflow="${WORKFLOW}" \
        --json databaseId,status,headBranch,createdAt \
        --jq '[.[] | select(.status=="completed")][0] | "\(.databaseId)"')

    if [ -z "${RUN_ID}" ]; then
        echo "ERROR: No completed '${WORKFLOW}' run found." >&2
        echo "       Trigger one with: gh workflow run 'Test Build' --ref <branch>" >&2
        exit 1
    fi

    # Show some info about the run.
    gh run list \
        --workflow="${WORKFLOW}" \
        --json databaseId,status,headBranch,createdAt,displayTitle \
        --jq "[.[] | select(.status==\"completed\")][0] | \"Run #\(.databaseId) — branch=\(.headBranch) created=\(.createdAt)\""
fi

echo ""
echo "Downloading artifacts → ${OUT_DIR}/"
rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

gh run download "${RUN_ID}" --dir "${OUT_DIR}"

echo ""
echo "Done. Contents of ${OUT_DIR}/:"
find "${OUT_DIR}" -maxdepth 2 | sort
