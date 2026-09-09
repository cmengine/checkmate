fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = std::fs::read_to_string(&args[1]).unwrap();
    match cme_compiler::mega::expand::expand_source(&src) {
        Ok(outcome) => println!("EXPANDED:\n{}", outcome.expanded),
        Err(diags) => {
            for d in &diags {
                println!("ERR: {}", d.message());
            }
        }
    }
}
