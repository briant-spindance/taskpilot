#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Clean previous output
rm -rf test/output
mkdir -p test/output

echo "=== taskpilot end-to-end test ==="
echo ""

# Dry run first
echo "--- Dry Run ---"
TASKPILOT_SKILLS_DIR=./test/skills taskpilot run \
  --prompt-file ./test/prompt.md \
  --input ./test/input/sales_data.csv \
  --output ./test/output \
  --dry-run

echo ""
echo "--- Live Run ---"

TASKPILOT_SKILLS_DIR=./test/skills taskpilot run \
  --prompt-file ./test/prompt.md \
  --input ./test/input/sales_data.csv \
  --output ./test/output

echo ""
echo "--- Output ---"

if [ -f test/output/report.md ]; then
  echo "✓ report.md generated ($(wc -l < test/output/report.md) lines)"
  echo ""
  cat test/output/report.md
else
  echo "✗ report.md not found in test/output/"
  ls -la test/output/
  exit 1
fi
