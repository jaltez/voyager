# Recipes

Copy-paste starting points. All assume machine output; add `-vv` to debug.

## Compare A vs B (the core deep-research use)

```sh
vygr plan "A vs B: performance, maturity, ecosystem"          # free preflight
vygr research "A vs B: performance, maturity, ecosystem. \
Include evidence against each side." --llm pi --budget-usd 0.50
```

Then read `answer.md` from the run dir and keep its `[n]` citations.

## Structured extraction for downstream code

```sh
cat > /tmp/schema.json <<'EOF'
{
  "type": "object",
  "required": ["recommendation", "pros", "cons"],
  "properties": {
    "recommendation": {"type": "string"},
    "pros": {"type": "array", "items": {"type": "string"}},
    "cons": {"type": "array", "items": {"type": "string"}}
  }
}
EOF
vygr research "which message queue for our throughput?" --llm pi \
  --output-schema /tmp/schema.json --format json
```

`schema_valid` in the JSON report tells you whether the answer parsed.

## Cheap deep dive (local or low-cost model)

```sh
vygr research "topic" --llm ollama:qwen3:8b --depth 2..3 --breadth 3
vygr research "topic" --llm pi --depth 1..2 --budget-usd 0.10   # quick pass
```

## Keep artifacts out of the repo

```sh
vygr research "topic" --llm pi --run-dir /tmp/vygr-runs
vygr runs list                       # still discovers runs under configured dirs
```

Or set `VGR_RUN_DIR=/tmp/vygr-runs` once per session.

## Pipe results into other tools

```sh
vygr search "rust wasm" --format jsonl | jq -r '.url'
vygr search "rust wasm" --format urls | head -3 | xargs -I{} vygr extract {} --max-chars 8000
```

## Resume / re-synthesize without new searches

```sh
vygr runs list --format json
vygr runs show 20261106-            # print answer.md of the newest match
vygr runs resume 20261106 --llm pi  # different model, same evidence
```

## Register as MCP server (do it once per harness)

```sh
vygr init --agent claude-code --mcp   # writes/merges ./.mcp.json
pi mcp add voyager -- vygr serve
```

Then the harness gets `search`, `extract`, `research`, `get_artifact` as
native tools with progress notifications during long runs.

## Verify an install end to end

```sh
vygr doctor --format json   # providers, LLM, dirs, skill freshness
vygr schema | jq '.commands.research.args'   # authoritative flags
```
