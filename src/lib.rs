/// Returns the embedded whitepaper.
pub fn whitepaper() -> &'static str {
    include_str!("../whitepaper.md")
}
