use std::io;

trait ProcessLimits {
    fn apply() -> io::Result<()>;
}

struct PlatformLimits;

#[cfg(target_os = "linux")]
impl ProcessLimits for PlatformLimits {
    fn apply() -> io::Result<()> {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        for (resource, ceiling) in [
            (Resource::As, super::MEMORY_BYTES),
            (Resource::Cpu, super::CPU_SECONDS),
            (Resource::Core, 0),
        ] {
            let existing = getrlimit(resource);
            let ceiling = existing
                .maximum
                .map_or(ceiling, |maximum| maximum.min(ceiling));
            let ceiling = existing
                .current
                .map_or(ceiling, |current| current.min(ceiling));
            setrlimit(
                resource,
                Rlimit {
                    current: Some(ceiling),
                    maximum: Some(ceiling),
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
impl ProcessLimits for PlatformLimits {
    fn apply() -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "contained GPX parsing is not implemented on this platform",
        ))
    }
}

pub(super) fn apply() -> io::Result<()> {
    PlatformLimits::apply()
}
