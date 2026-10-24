use app::ops::OPS;

use super::COMMANDS;
use crate::args::Form;

// Each read operation is one subcommand, under its own name, in the reference order of OPS.
#[test]
fn each_read_operation_is_the_subcommand_of_its_name() {
    let read: Vec<&str> = COMMANDS
        .iter()
        .filter_map(|command| match command.form {
            Form::Read(op) => {
                assert_eq!(command.name, op.name);
                assert_eq!(command.help, op.summary);
                Some(op.name)
            }
            Form::Built(_) => None,
        })
        .collect();
    let declared: Vec<&str> = OPS.iter().map(|op| op.name).collect();
    assert_eq!(read, declared);
}
