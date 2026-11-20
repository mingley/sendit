fn main() {
    if let Err(error) = sendit_server::run_dedicated() {
        eprintln!("sendit-server: {error}");
        std::process::exit(1);
    }
}
