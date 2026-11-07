//! `vygr schema`: machine-readable self-description so LLM agents can
//! drive the CLI without `--help`. The command tree is derived from clap
//! itself (`crate::cli_command()`), so it cannot drift from the real
//! interface; only examples and the static contract sections live here
//! (hsearch-inspired, ADR-0008/0016).

use vygr_core::VygrError;

use crate::cli_command;

pub fn run() -> Result<(), VygrError> {
    println!("{}", serde_json::to_string_pretty(&build_schema())?);
    Ok(())
}

/// Assemble the full schema: clap-derived command tree + static contract
/// sections (machine contract, artifacts, LLM specs, examples).
pub(crate) fn build_schema() -> serde_json::Value {
    let cmd = cli_command();
    let mut commands = serde_json::Map::new();
    for sub in cmd.get_subcommands() {
        commands.insert(
            sub.get_name().to_string(),
            command_json(sub, &example_map()),
        );
    }
    serde_json::json!({
        "name": cmd.get_name(),
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Configurable deep research CLI: provider-chain web search, page extraction, and full research runs over any LLM backend.",
        "usage_hint": "This schema is authoritative for flags and formats; it is derived from the CLI itself. Run `vygr <command> --help` only for wrapped prose.",
        "commands": commands,
        "machine_contract": machine_contract(),
        "artifacts": artifacts(),
        "provider_chains": "comma-separated fallback (first provider with results wins); `--all` queries every ready provider concurrently and dedups by URL",
        "llm_specs": [
            "pi", "claude", "codex",
            "ollama:<model>",
            "openai-compat (with [llm] base_url + model)",
            "<models.dev provider>:<model>  e.g. openrouter:anthropic/claude-sonnet-4.5"
        ],
        "config": {
            "precedence": "CLI flags > VGR_DEFAULT_PROVIDER/VGR_LLM_BACKEND/VGR_LLM_MODEL env > project .voyager.toml > ~/.config/voyager/config.toml > defaults",
            "keys_are_env_only": "API keys are never stored in config files; providers read their own env vars (see `vygr providers`)",
            "sections_merge_per_field": "a config file section only overrides the fields it sets; other layers' values survive"
        }
    })
}

/// One command (or subcommand group): about, args, nested subcommands.
fn command_json(cmd: &clap::Command, examples: &ExampleMap) -> serde_json::Value {
    let args: Vec<serde_json::Value> = cmd
        .get_arguments()
        .filter(|a| {
            let id = a.get_id().as_str();
            id != "help" && id != "version"
        })
        .map(arg_json)
        .collect();
    let name = cmd.get_name().to_string();
    let subs: Vec<serde_json::Value> = cmd
        .get_subcommands()
        .map(|sub| command_json(sub, examples))
        .collect();
    let mut out = serde_json::json!({
        "name": name,
        "about": cmd.get_about().map(|s| s.to_string()),
        "args": args,
    });
    if !subs.is_empty() {
        out["subcommands"] = serde_json::Value::Array(subs);
    }
    if let Some(ex) = examples.get(name.as_str()) {
        if !ex.is_empty() {
            out["examples"] = serde_json::json!(ex);
        }
    }
    out
}

/// One argument: kind (positional / option / flag), choices for enums,
/// defaults, required-ness, help text.
fn arg_json(a: &clap::Arg) -> serde_json::Value {
    let kind = if a.is_positional() {
        "positional"
    } else if a.get_action().takes_values() {
        "option"
    } else {
        "flag"
    };
    let choices: Vec<String> = a
        .get_possible_values()
        .iter()
        .map(|v| v.get_name().to_string())
        .collect();
    serde_json::json!({
        "name": a.get_long().map(str::to_string).unwrap_or_else(|| a.get_id().to_string()),
        "kind": kind,
        "short": a.get_short().map(|c| c.to_string()),
        "required": a.is_required_set(),
        "multiple": matches!(a.get_action(), clap::ArgAction::Append | clap::ArgAction::Count),
        "default": a.get_default_values().iter().map(|v| v.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "choices": if choices.is_empty() { None } else { Some(choices) },
        "help": a.get_help().map(|s| s.to_string()),
    })
}

fn machine_contract() -> serde_json::Value {
    serde_json::json!({
        "stdout": "data only; every diagnostic goes to stderr",
        "formats": "pass --format json (or jsonl where offered) on every command for machine output; table/md/urls are for humans",
        "stdin": "a query of `-` reads the query from stdin",
        "exit_codes": {
            "0": "success",
            "1": "internal error",
            "2": "usage error",
            "3": "configuration or auth error",
            "4": "provider or network failure"
        },
        "errors": "stderr lines are prefixed `vygr:`; JSON modes never mix diagnostics into stdout"
    })
}

fn artifacts() -> serde_json::Value {
    serde_json::json!({
        "run_dir": "agents/voyager/<UTC-timestamp>-<slug>/ (override: --run-dir, VGR_RUN_DIR, or [research] run_dir)",
        "files": {
            "prompt.md": "the exact prompts issued to the LLM",
            "plan.json": "planned sub-queries and depth",
            "reflections.json": "per-level distilled notes, follow-ups and disagreements",
            "sources.json": "scored sources incl. fetched content; [n] citations index into this order",
            "claims.json": "claim-level verification table (claim, citation, best source, verdict)",
            "answer.md": "the synthesized report with [n] citations",
            "run.json": "manifest: query, depth, iterations, cost, warnings, parent link"
        },
        "cite": "when asked where a conclusion came from, cite these files and the [n] entries in sources.json"
    })
}

type ExampleMap = std::collections::BTreeMap<&'static str, Vec<&'static str>>;

fn example_map() -> ExampleMap {
    ExampleMap::from([
        (
            "search",
            vec![
                "vygr search \"rust async runtime comparison\" --max-results 5",
                "vygr search \"q\" --extract-top 3 --format json",
                "vygr search \"q\" --all --format jsonl",
                "echo \"query from stdin\" | vygr search - --format urls",
            ],
        ),
        (
            "extract",
            vec!["vygr extract https://example.com/a --max-chars 12000 --format json"],
        ),
        (
            "research",
            vec![
                "vygr plan \"question\"",
                "vygr research \"question\" --llm pi --mode deep --events",
                "vygr research \"question\" --llm ollama:qwen3:8b --depth 2..4 --budget-usd 0.50",
                "vygr research \"question\" --llm pi --run-dir /tmp/vygr-runs",
            ],
        ),
        (
            "runs",
            vec![
                "vygr runs list --format json",
                "vygr runs show 20261106-",
                "vygr runs show 20261106- --export report.docx",
                "vygr runs show 20261106- --export report.pdf",
                "vygr runs resume 20261106 --llm pi",
            ],
        ),
        (
            "serve",
            vec![
                "pi mcp add voyager -- vygr serve",
                "vygr serve --http 127.0.0.1:8080 --token <secret>",
            ],
        ),
        ("doctor", vec!["vygr doctor --format json"]),
        (
            "eval",
            vec!["vygr eval 20261106-143210-topic --format json"],
        ),
        (
            "followup",
            vec![
            "vygr followup 20261106-143210-topic \"what changed for windows support?\" --llm pi",
        ],
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_exposes_full_flag_trees() {
        let schema = build_schema();
        let search = &schema["commands"]["search"];
        assert!(search["about"].is_string());
        let args = search["args"].as_array().unwrap();
        let longs: Vec<&str> = args.iter().filter_map(|a| a["name"].as_str()).collect();
        for expected in ["provider", "all", "max-results", "format", "time-range"] {
            assert!(
                longs.contains(&expected),
                "schema for search is missing `{expected}` (got {longs:?})"
            );
        }
        // Format choices come from the ValueEnum, so new formats surface
        // here automatically.
        let fmt = args
            .iter()
            .find(|a| a["name"] == "format")
            .expect("format arg");
        let choices: Vec<&str> = fmt["choices"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c.as_str())
            .collect();
        assert!(
            choices.contains(&"json"),
            "format choices lack json: {choices:?}"
        );
    }

    #[test]
    fn schema_lists_every_machine_contract_exit_code() {
        let schema = build_schema();
        let codes = schema["machine_contract"]["exit_codes"]
            .as_object()
            .unwrap();
        for code in ["0", "1", "2", "3", "4"] {
            assert!(codes.contains_key(code), "schema exit_codes missing {code}");
        }
    }

    #[test]
    fn schema_covers_subcommand_groups_and_examples() {
        let schema = build_schema();
        let runs = &schema["commands"]["runs"];
        assert!(
            runs["subcommands"].as_array().map(|s| s.len()).unwrap_or(0) >= 3,
            "runs subcommands missing from schema"
        );
        assert!(
            schema["commands"]["search"]["examples"]
                .as_array()
                .is_some(),
            "search examples missing from schema"
        );
        // Static sections stay present.
        assert!(schema["artifacts"]["files"]["sources.json"].is_string());
        assert!(schema["llm_specs"].as_array().unwrap().len() >= 4);
    }
}
