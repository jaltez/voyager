//! voyager research engine (ADR-0005): plan-then-execute loop over the
//! search providers, context assembly and synthesis through any
//! [`vygr_llm::LlmClient`] backend.

mod cluster;
mod eval;
mod mode;
mod orchestrator;
mod planner;
mod run_dir;
mod runs;
mod score;
mod verify;

pub use eval::{evaluate, EvalReport};
pub use mode::ResearchMode;
pub use orchestrator::{
    run, DepthSpec, EventSink, ParentContext, ProgressSink, ResearchEvent, ResearchReport,
    ResearchRequest, Source,
};
pub use run_dir::create_run_dir;
pub use runs::{default_base, list_runs, resolve_run, resynthesize, RunInfo, RunManifest};
pub use verify::{verify, Verification};
