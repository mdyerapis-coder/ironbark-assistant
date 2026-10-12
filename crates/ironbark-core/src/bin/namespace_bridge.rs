//! Stage this binary and its exact ELF dependencies inside the private worker namespace.
//! Invocation: /run/ironbark-net/namespace-bridge --bootstrap-fd <native-inherited-fd>
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fd = match args.as_slice() {
        [flag, value] if flag == "--bootstrap-fd" => value.parse::<i32>().ok().filter(|fd| *fd >= 3),
        _ => None,
    };
    let Some(fd) = fd else { std::process::exit(64); };
    if ironbark_core::net::namespace_bridge::run_from_fd(fd).await.is_err() {
        // No endpoints, headers, provider payloads or bootstrap bytes in diagnostics.
        eprintln!("private namespace bridge unavailable");
        std::process::exit(1);
    }
}
