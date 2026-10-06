#[path = "support/compare_gray_impl.rs"]
mod implementation;

fn main() {
    match implementation::run("validation-results/gray.json", "gray") {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("invalid offline gray manifest: {error}");
            std::process::exit(2);
        }
    }
}
