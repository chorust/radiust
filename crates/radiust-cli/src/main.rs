use std::process::ExitCode;

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect();
    ExitCode::from(radiust_cli::run_args(args))
}
