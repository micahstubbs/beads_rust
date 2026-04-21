# Errors

Most commands return non-zero exit codes on failure and may emit a structured error envelope. With JSON output enabled, successful data is written to stdout; structured errors are written to stderr. Parse stdout only after exit code `0`, and parse stderr after non-zero exits.

Example (captured with stderr redirection):

```bash
br show bd-NOTEXIST --format json > /dev/null 2>err.json || true
cat err.json | jq .
```

Minimal regression check:

```bash
set +e
br show bd-NOTEXIST --json >out.json 2>err.json
status=$?
set -e
test "$status" -eq 3
test ! -s out.json
jq -e '.error.code == "ISSUE_NOT_FOUND"' err.json >/dev/null
```

Shape:

```json
{
  "error": {
    "code": "ISSUE_NOT_FOUND",
    "message": "Issue not found: bd-NOTEXIST",
    "hint": "Run 'br list' to see available issues.",
    "retryable": false,
    "context": { "searched_id": "bd-NOTEXIST" }
  }
}
```

Machine-readable schema:

```bash
br schema error --format json
```
