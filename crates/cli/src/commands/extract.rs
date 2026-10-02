//! `vygr extract`: fetch URLs and reduce them to plain text.

use clap::Args as ClapArgs;
use futures::future::join_all;
use vygr_core::provider::FetchProvider;
use vygr_core::types::FetchPage;
use vygr_core::VygrError;

use crate::output::{render_pages, ExtractFormat};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// URLs to fetch (http/https)
    pub urls: Vec<String>,

    #[arg(long, value_enum, default_value = "text")]
    pub format: ExtractFormat,

    /// Maximum characters of extracted text per page
    #[arg(long, default_value_t = 20_000)]
    pub max_chars: usize,
}

pub async fn run(args: Args, http: reqwest::Client) -> Result<(), VygrError> {
    if args.urls.is_empty() {
        return Err(VygrError::Config("no URLs given".to_string()));
    }
    let fetcher = vygr_providers::HttpFetch::new(http);
    let pages = join_all(args.urls.iter().map(|u| fetcher.fetch(u, args.max_chars))).await;

    let mut ok: Vec<FetchPage> = Vec::new();
    for (url, page) in args.urls.iter().zip(pages) {
        match page {
            Ok(p) => ok.push(p),
            Err(e) => {
                tracing::warn!("{e}");
                eprintln!("vygr: skipping {url}: {e}");
            }
        }
    }
    if ok.is_empty() {
        return Err(VygrError::provider("http", "every fetch failed"));
    }
    println!("{}", render_pages(args.format, &ok));
    Ok(())
}
