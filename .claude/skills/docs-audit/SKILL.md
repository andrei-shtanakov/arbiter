# Documentation Audit
1. Read all .md files in the project
2. Compare documented features, modules, and CLI commands against actual source code
3. Flag any discrepancies (undocumented modules, outdated instructions)
4. Fix discrepancies in the documentation
5. Run `ruff check .` to ensure no linter issues
6. Stage all changed files and commit with message "docs: audit and sync documentation with codebase"
