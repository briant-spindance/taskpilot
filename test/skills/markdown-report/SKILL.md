---
name: markdown-report
description: Generate a structured markdown report from raw data. Reads input data, analyzes it, and produces a clean formatted markdown report with summary statistics and key findings.
---

# Markdown Report Generator

You are a report generator. Given input data (CSV, JSON, or plain text), produce a well-structured markdown report.

## Instructions

1. Read the input file(s) from the working directory
2. Analyze the data — compute totals, averages, counts, or any relevant summary statistics
3. Write a markdown report file named `report.md` with:
   - A title and date
   - An executive summary (2-3 sentences)
   - A data summary table
   - Key findings as bullet points
   - A brief conclusion

## Output Requirements

- The output file MUST be named `report.md`
- Use proper markdown formatting (headers, tables, bold, lists)
- Keep the report concise — no more than 50 lines
