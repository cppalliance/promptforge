//! Linux process identity lookup: `/proc/<pid>/exe` supplies the image and
//! field 22 of `/proc/<pid>/stat` supplies the kernel start tick.

use super::ProcessIdentity;

/// The image and start tick for process `pid`, or `None` when the process
/// disappears, changes identity during observation, or `/proc` is unreadable.
pub(crate) fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    let first_start = process_start(pid)?;
    let image = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let second_start = process_start(pid)?;
    (first_start == second_start).then(|| ProcessIdentity::new(image, u128::from(first_start)))
}

/// Reads Linux `/proc/<pid>/stat` field 22. The command field is enclosed
/// in parentheses and may itself contain spaces or closing parentheses,
/// so fields are counted only after its final delimiter.
fn process_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_command = stat.get(stat.rfind(')')? + 1..)?;
    after_command.split_whitespace().nth(19)?.parse().ok()
}
