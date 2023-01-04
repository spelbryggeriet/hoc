use clap::Parser;

#[derive(Parser)]
pub struct Defaults {
    /// Skip prompts for fields that have defaults
    ///
    /// This is equivalent to providing all defaultable flags without a value.
    #[clap(short, long)]
    defaults: bool,
}
