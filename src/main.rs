#[allow(dead_code)]
mod cli {
    include!("cli.rs");
    include!("codex_dashboard_1.rs");
    include!("codex_dashboard_2.rs");
    include!("codex_dashboard_3.rs");
    include!("codex_dashboard_4.rs");

    pub(super) fn entry() {
        if let Err(error) = run_with_codex_dashboard() {
            eprintln!("error: {error:#}");
            std::process::exit(1);
        }
    }
}

fn main() {
    cli::entry();
}
