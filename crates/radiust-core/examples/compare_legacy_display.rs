#[path = "support/compare_gray_impl.rs"]
mod implementation;

fn main() {
    match implementation::run(
        "validation-results/legacy-display.json",
        "legacy display compatibility",
    ) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("invalid offline display manifest: {error}");
            std::process::exit(2);
        }
    }
}
