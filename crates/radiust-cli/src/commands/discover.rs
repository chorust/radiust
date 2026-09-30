use super::Context;
use crate::report;

pub(crate) struct Args {
    pub sources: Vec<String>,
    pub product: Option<String>,
    pub stations: Vec<String>,
    pub latest: bool,
    pub at: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub base_time: Option<String>,
    pub max_age_secs: Option<f64>,
}

pub(crate) fn run(args: Args, context: &Context<'_>) -> Result<u8, String> {
    let query = crate::build_query(
        args.sources.clone(),
        args.product,
        args.stations,
        args.latest,
        args.at,
        args.start,
        args.end,
        args.base_time,
        args.max_age_secs,
    )?;
    let config = crate::load_config(context.config_path)?;
    let result = crate::discover(config, query, context.progress_enabled)
        .map_err(|error| error.to_string())?;
    if args.sources.len() > 1 || args.sources[0] == "all" {
        let exit_code = crate::discovery_exit_code(&result);
        crate::emit_cli(
            &report::discovery_envelope(&result),
            context.json,
            context.quiet,
            context.verbose,
            Some("discover"),
        )?;
        Ok(exit_code)
    } else {
        let (payload, exit_code) = crate::single_discovery_payload(&result)?;
        // The legacy single-source JSON projects successful frames only. Human
        // reports need the complete discovery result to show partial failures.
        let human_payload = report::discovery_envelope(&result);
        crate::emit_cli(
            if context.json { &payload } else { &human_payload },
            context.json,
            context.quiet,
            context.verbose,
            Some("discover"),
        )?;
        Ok(exit_code)
    }
}
