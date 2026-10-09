use crate::view::*;
use nddev_device_sync_adapter_io::{CancellationToken, IoError, ProcessIo, ProcessRequest};

use std::{path::PathBuf, time::Duration};

#[derive(Clone)]
pub struct Rds {
    io: ProcessIo,
    executable: PathBuf,
    control_directory: Option<PathBuf>,
}

impl Rds {
    pub fn new(
        io: ProcessIo,
        executable: PathBuf,
        control_directory: Option<PathBuf>,
    ) -> Result<Self, IoError> {
        if !executable.is_absolute()
            || control_directory
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
        {
            return Err(IoError::InvalidConfiguration);
        }
        Ok(Self {
            io,
            executable,
            control_directory,
        })
    }

    pub async fn sessions(
        &self,
        cancellation: CancellationToken,
    ) -> Result<RemoteSnapshot, IoError> {
        let mut arguments = vec!["session".into(), "list".into(), "--json".into()];
        if let Some(directory) = &self.control_directory {
            arguments.extend(["--control-dir".into(), directory.as_os_str().into()]);
        }
        let request = ProcessRequest {
            executable: self.executable.clone(),
            arguments,
            directory: None,
            timeout: Duration::from_secs(5),
            max_output_bytes: 65_536,
        };
        let (snapshot, _): (RemoteSnapshot, _) = self.io.json(request, &[0], cancellation).await?;
        if !hex(&snapshot.instance, 32)
            || !hex(&snapshot.endpoint, 64)
            || snapshot.sessions.len() > 32
            || snapshot
                .selected
                .as_ref()
                .is_some_and(|id| !snapshot.sessions.iter().any(|session| &session.id == id))
            || snapshot
                .sessions
                .iter()
                .any(|session| !hex(&session.id, 32) || !hex(&session.peer, 64))
        {
            return Err(IoError::InvalidResponse);
        }
        let mut ids = std::collections::BTreeSet::new();
        if snapshot
            .sessions
            .iter()
            .any(|session| !ids.insert(&session.id))
        {
            return Err(IoError::InvalidResponse);
        }
        Ok(snapshot)
    }
}

fn hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
