//! Process-lifecycle helpers shared by Packet28's long-lived background
//! processes (`packet28d serve`, `Packet28 hook serve-http`).

/// Outcome of [`detach_from_parent_session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    /// The process now leads its own session and process group.
    Detached,
    /// The process already led a process group (typically started directly
    /// from an interactive shell), so `setsid` was not applicable.
    AlreadyLeader,
    /// Detaching is not supported on this platform.
    Unsupported,
}

/// Move the current process into a fresh session so it no longer belongs to
/// the process group (or controlling terminal) of whoever spawned it.
///
/// Packet28 background processes are usually spawned from short-lived hook
/// invocations. Hosts such as Claude Code tear down a finished hook's process
/// group, and terminals send `SIGHUP` to their session on close; without this
/// call the daemon or HTTP hook server is killed along with the hook that
/// started it and the next hook simply starts another one.
#[cfg(unix)]
pub fn detach_from_parent_session() -> DetachOutcome {
    // SAFETY: `getpid`, `getpgrp` and `setsid` take no pointers and only
    // mutate this process's own session/process-group membership.
    unsafe {
        if libc::getpgrp() == libc::getpid() {
            return DetachOutcome::AlreadyLeader;
        }
        if libc::setsid() == -1 {
            return DetachOutcome::AlreadyLeader;
        }
    }
    DetachOutcome::Detached
}

#[cfg(not(unix))]
pub fn detach_from_parent_session() -> DetachOutcome {
    DetachOutcome::Unsupported
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Fork-free check: a spawned child (which is *not* a group leader)
    /// must end up in its own session and process group after detaching.
    #[test]
    fn spawned_child_detaches_into_its_own_session() {
        use std::process::Command;

        // SAFETY: getpgrp reads process metadata and takes no pointers.
        let parent_pgid = unsafe { libc::getpgrp() };
        // Re-run this test binary as a child that only performs the detach
        // and prints its resulting pgid/sid.
        let output = Command::new(std::env::current_exe().unwrap())
            .env("PACKET28_DETACH_CHILD", "1")
            .args([
                "--nocapture",
                "--exact",
                "process::tests::detach_child_probe",
            ])
            .output()
            .expect("spawn child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|line| line.starts_with("DETACH "))
            .unwrap_or_else(|| panic!("child did not report detach: {stdout}"));
        let mut parts = line.split_whitespace().skip(1);
        let outcome = parts.next().unwrap();
        let pid: i32 = parts.next().unwrap().parse().unwrap();
        let pgid: i32 = parts.next().unwrap().parse().unwrap();
        let sid: i32 = parts.next().unwrap().parse().unwrap();

        assert_eq!(outcome, "Detached");
        assert_eq!(pgid, pid, "child should lead its own process group");
        assert_eq!(sid, pid, "child should lead its own session");
        assert_ne!(pgid, parent_pgid, "child must leave the parent's group");
    }

    #[test]
    fn detach_child_probe() {
        if std::env::var_os("PACKET28_DETACH_CHILD").is_none() {
            return;
        }
        let outcome = detach_from_parent_session();
        // SAFETY: these calls only read this process's identity and take no pointers.
        let (pid, pgid, sid) = unsafe { (libc::getpid(), libc::getpgrp(), libc::getsid(0)) };
        println!("DETACH {outcome:?} {pid} {pgid} {sid}");
    }
}
