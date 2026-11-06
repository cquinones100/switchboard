//! `wait-deploy`: Wait for a commit's deploy. Ported from `claude/skills/deploy-test/scripts/wait-deploy.js`.

use std::process::ExitCode;

pub fn main(_args: Vec<String>) -> ExitCode {
    super::not_yet("wait-deploy")
}
