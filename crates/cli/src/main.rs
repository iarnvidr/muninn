fn main() {
    if let Err(e) = muninn_cli::run(std::env::args().skip(1).collect()) {
        eprintln!("muninn: {e}");
        std::process::exit(1);
    }
}
