//! Fixture-directory probe for the E-02 subprocess test (P15-07).
//!
//! Reads `$MC_FIXTURE_DIR` exactly the way production startup does — through
//! [`mc_registry::Registries::vanilla`] — and reports the outcome on stdout:
//! `OK blocks=<n>` on success, `ERR <message>` on failure. Exit status mirrors
//! the outcome (2 when the variable is absent entirely). The parent test drives
//! this binary with controlled environments because process-global state cannot
//! be set safely from inside a multithreaded test harness.

fn main() {
    let override_dir = std::env::var("MC_FIXTURE_DIR").unwrap_or_else(|_| {
        eprintln!("MC_FIXTURE_DIR is not set");
        std::process::exit(2);
    });
    match mc_registry::Registries::vanilla() {
        Ok(registries) => {
            println!(
                "OK blocks={} dir={override_dir}",
                registries.blocks.state_count()
            );
        }
        Err(error) => {
            println!("ERR {error}");
            std::process::exit(1);
        }
    }
}
