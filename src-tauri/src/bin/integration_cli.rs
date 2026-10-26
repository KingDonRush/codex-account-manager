fn main() {
    if let Err(error) = codex_account_manager_lib::run_integration_cli() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
