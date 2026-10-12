// bwrap launches this binary as the namespace controller with --as-pid-1.
// The controller must run before any Tokio runtime exists (single-threaded,
// PID1 signal handler, seccomp TSYNC); the daemon serve path is untouched.
fn main() {
    if let Some(code) = ironbark_core::process::namespace_runner_entry() {
        std::process::exit(code);
    }
    let result = match ironbark_core::paths::RuntimePaths::from_env() {
        Ok(paths) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(ironbark_core::Core::serve(paths)),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
