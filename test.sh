#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Clean previous output
rm -rf test/output test/staging
mkdir -p test/output test/staging

echo "=== taskpilot end-to-end test ==="
echo ""

# Dry run first
echo "--- Dry Run ---"
TASKPILOT_SKILLS_DIR=./test/skills taskpilot run sales-report --dry-run

echo ""
echo "--- Live Run (full chain: clean-data → sales-report) ---"

TASKPILOT_SKILLS_DIR=./test/skills taskpilot run sales-report

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

echo ""
echo "--- Executive Summary (depends on sales-report) ---"

TASKPILOT_SKILLS_DIR=./test/skills taskpilot run executive-summary

if [ -f test/output/executive-summary.md ]; then
  echo "✓ executive-summary.md generated ($(wc -l < test/output/executive-summary.md) lines)"
  echo ""
  cat test/output/executive-summary.md
else
  echo "✗ executive-summary.md not found in test/output/"
  ls -la test/output/
  exit 1
fi
