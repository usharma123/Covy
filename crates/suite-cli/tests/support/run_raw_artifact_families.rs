use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

pub struct FakeReducerBins {
    _bin_dir: TempDir,
    path_env: String,
}

impl FakeReducerBins {
    pub fn path_env(&self) -> &str {
        &self.path_env
    }
}

#[cfg(unix)]
pub fn install_fake_reducer_bins() -> FakeReducerBins {
    let bin_dir = TempDir::new().unwrap();
    write_executable_script(
        &bin_dir.path().join("cargo"),
        "#!/bin/sh\nprintf '    Checking packet28_fixture v0.1.0\\n    Finished dev [unoptimized + debuginfo] target(s) in 0.01s\\nrust raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("npx"),
        "#!/bin/sh\nprintf 'javascript raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("python3"),
        "#!/bin/sh\nprintf 'tests/test_demo.py .\\n1 passed in 0.01s\\npython raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("go"),
        "#!/bin/sh\nprintf 'ok\\tpacket28.test\\t0.01s\\ngo raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("docker"),
        "#!/bin/sh\nprintf 'infra raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gh"),
        "#!/bin/sh\nprintf 'build\\tpass\\t1s\\ngithub raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gt"),
        "#!/bin/sh\nprintf 'Pushed branch feat/add-auth\\nCreated pull request #42 for feat/add-auth\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("ruby"),
        "#!/bin/sh\nprintf '1 runs, 1 assertions, 0 failures, 0 errors, 0 skips\\nruby raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("dotnet"),
        "#!/bin/sh\nprintf 'Passed!  - Failed: 0, Passed: 1, Skipped: 0, Total: 1, Duration: 1 s\\ndotnet raw marker\\n'\n",
    );
    write_executable_script(
        &bin_dir.path().join("gradle"),
        "#!/bin/sh\nprintf 'ExampleTest > fails FAILED\\n    java.lang.AssertionError: expected true\\n        at org.junit.Assert.fail(Assert.java:89)\\n        at com.example.ExampleTest.fails(ExampleTest.java:42)\\n2 tests completed, 1 failed\\nBUILD FAILED in 1s\\ngradle raw marker\\n'\n",
    );
    let path_env = format!(
        "{}:{}",
        bin_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    FakeReducerBins {
        _bin_dir: bin_dir,
        path_env,
    }
}

#[cfg(unix)]
fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}
