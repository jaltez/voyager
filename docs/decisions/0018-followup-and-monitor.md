# ADR 0018: Follow-up runs and monitors (cross-run memory)

- **Date:** 2026-11-06
- **Status:** Accepted (implemented in 0.7.0)
- **Background:** "cross-run memory / recurring research with trend
  snapshots" sat in the ROADMAP's deferred list for two releases. The
  blocking question was scope: a memory system (embeddings, indexes) is
  out of proportion for a single binary. The follow-up mechanism needs
  none of it — every run already leaves its full evidence on disk.

## Decision

1. **`ParentContext` is the whole memory system.** A follow-up run
   receives the parent's distilled notes, its sources (incl. content)
   and its URL set. The loop seeds the visited set with parent URLs (no
   refetching), starts with the parent's notes, and hands the planner
   the notes so sub-queries target what is missing. Synthesis is told to
   emphasize what is NEW or CHANGED relative to the parent.
2. **`vygr followup <run> "question"`.** Resolves a run by name prefix,
   loads `sources.json` + `reflections.json` + `run.json`, and starts a
   normal run with `parent` set. When no fresh sources turn up, the run
   does not fail (unlike a cold start) — it synthesizes from the
   inherited evidence.
3. **`vygr monitor add|list|run`.** A monitor is a JSON file under
   `<data_dir>/voyager/monitors/<name>.json`: `{name, topic, last_run,
   updated_unix}`. `monitor run` behaves like `followup` on `last_run`
   (a fresh check when the baseline is gone) and advances `last_run` to
   the new run afterwards. The "trend snapshot" is the pair of runs on
   disk plus the parent-aware synthesis; no separate trend store.
4. **Provenance.** Follow-up runs write `parent.json` (inherited source
   and note counts) and record `parent` in `run.json`.

## Consequences

- Cross-run continuity works with zero new storage beyond one small
  state file per monitor; the evidence trail (ADR-0009) remains the
  single source of truth.
- Parent sources keep their stored scores; ranking mixes parent and
  fresh evidence by BM25 only for new sources. Acceptable: parents are
  context, fresh findings should compete for attention.
- Monitors are per-user (data dir), not per-project; a project that
  wants shared monitors can keep a `.voyager.toml` `[research]` setup
  and script `followup` instead.
