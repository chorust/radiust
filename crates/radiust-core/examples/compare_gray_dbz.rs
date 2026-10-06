#[path = "support/compare_gray_dbz_impl.rs"]
mod implementation;

fn main() {
    match implementation::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("invalid offline gray/dBZ manifest: {error}");
            std::process::exit(2);
        }
    }
}
