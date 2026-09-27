fn main() {
    let code = match worldstream_agent_swarm::execution::process::run_guard_stdio() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    std::process::exit(code);
}
