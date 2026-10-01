//! voyager research engine (ADR-0005): plan-then-execute loop over the
//! search providers, context assembly and synthesis through any
//! [`vygr_llm::LlmClient`] backend.

mod orchestrator;
mod planner;
mod run_dir;
mod score;

pub use orchestrator::{run, DepthSpec, ResearchReport, ResearchRequest};
pub use run_dir::create_run_dir;
