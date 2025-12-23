use crate::command::CommandRunner;
use clap::Parser;
use petname::Generator;

#[derive(Parser)]
#[command(name = "petname", about = "Generate a random petname")]
pub struct Petname;

impl CommandRunner for Petname {
    fn run(self) -> anyhow::Result<()> {
        let name = petname::Petnames::default()
            .generate_one(2, "-")
            .expect("couldn't generate name");
        println!("{}", name);
        Ok(())
    }
}
