//! WaOS port: `process::Child` for a WASI component, over `command`'s, which runs on the
//! machine the user picked: the same API as the other platforms'.

use anyhow::Result;
use std::process::Stdio;

pub struct Child {
    process: crate::command::Child,
}

impl std::ops::Deref for Child {
    type Target = crate::command::Child;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl std::ops::DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}

impl Child {
    pub fn spawn(
        command: std::process::Command,
        _stdin: Stdio,
        _stdout: Stdio,
        _stderr: Stdio,
    ) -> Result<Self> {
        let process = crate::command::from_std(&command).spawn()?;
        Ok(Self { process })
    }

    pub async fn output(self) -> Result<crate::command::Output> {
        Ok(self.process.output().await?)
    }

    pub fn kill(&mut self) -> Result<()> {
        Ok(self.process.kill()?)
    }
}
