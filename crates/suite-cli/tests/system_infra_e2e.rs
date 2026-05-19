use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

fn suite_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("Packet28")
}

#[cfg(unix)]
fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
#[cfg(unix)]
fn test_system_infra_and_count_commands_use_reducer_wrappers() {
    let root = TempDir::new().unwrap();
    let bin_dir = root.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(root.path().join("sample.txt"), "one\ntwo\nthree\n").unwrap();
    write_executable_script(
        &bin_dir.join("ls"),
        "#!/bin/sh\nprintf 'alpha.txt\\nbeta.txt\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("tree"),
        "#!/bin/sh\nprintf '.\\n|-- alpha.txt\\n`-- beta.txt\\n\\n0 directories, 2 files\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("wc"),
        "#!/bin/sh\nprintf '      3 sample.txt\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("wget"),
        "#!/bin/sh\nprintf '%s\\n' '--2026-05-12-- https://example.com/pkg.tgz' \"Saving to: 'pkg.tgz'\" \"'pkg.tgz' saved [2048/2048]\" >&2; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("curl"),
        "#!/bin/sh\nprintf '<html><title>Packet28 Fixture</title><body>ok</body></html>'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("docker"),
        "#!/bin/sh\nif [ \"$1\" = ps ]; then printf 'CONTAINER ID   IMAGE   STATUS\\nabc123         app     Up 2 minutes\\n'; exit 0; fi\nexit 2\n",
    );
    write_executable_script(
        &bin_dir.join("kubectl"),
        "#!/bin/sh\nif [ \"$1\" = get ]; then printf 'NAME   READY   STATUS\\napi    1/1     Running\\njob    0/1     Pending\\n'; exit 0; fi\nexit 2\n",
    );
    write_executable_script(
        &bin_dir.join("gh"),
        "#!/bin/sh\nif [ \"$1\" = pr ] && [ \"$2\" = list ]; then printf '12\\tFix packet routing\\tfeature\\n13\\tUpdate docs\\tmain\\n'; exit 0; fi\nexit 2\n",
    );
    write_executable_script(
        &bin_dir.join("glab"),
        "#!/bin/sh\nif [ \"$1\" = mr ] && [ \"$2\" = list ]; then printf '!7\\tOpen\\tAdd reducer\\n'; exit 0; fi\nexit 2\n",
    );
    write_executable_script(
        &bin_dir.join("aws"),
        "#!/bin/sh\nif [ \"$1\" = sts ] && [ \"$2\" = get-caller-identity ]; then printf '{\"Account\":\"123456789012\",\"Arn\":\"arn:aws:iam::123456789012:user/demo\",\"UserId\":\"AIDA\"}\\n'; exit 0; fi\nexit 2\n",
    );
    write_executable_script(
        &bin_dir.join("psql"),
        "#!/bin/sh\nprintf ' id | name\\n----+------\\n 1  | Ada\\n 2  | Lin\\n(2 rows)\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("git"),
        "#!/bin/sh\nif [ \"$1\" = status ]; then printf 'Changes not staged for commit:\\n\\tmodified:   src/lib.rs\\n\\nUntracked files:\\n\\tnew.txt\\n'; exit 0; fi\nexit 2\n",
    );
    write_executable_script(
        &bin_dir.join("gt"),
        "#!/bin/sh\nif [ \"$1\" = log ]; then printf '* abc123 feature-a\\n* def456 main\\n'; exit 0; fi\nexit 2\n",
    );
    let path_env = std::env::join_paths(std::iter::once(bin_dir.as_path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ls listed 2 entries in ."));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["tree"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "tree listed 0 dir(s), 2 file(s) under .",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["wc", "-l", "sample.txt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wc 3"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["wget", "https://example.com/pkg.tgz"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "wget example.com/pkg.tgz ok | pkg.tgz | 2.0KB",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["curl", "https://example.com"])
        .assert()
        .success()
        .stdout(predicate::str::contains("curl HTML 'Packet28 Fixture'"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["docker", "ps"])
        .assert()
        .success()
        .stdout(predicate::str::contains("docker ps listed 1 container"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["kubectl", "get", "pods"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "kubectl get pods: 2 row(s), 1 pending",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["gh", "pr", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gh pr list: 2 PR(s)"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["glab", "mr", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("glab mr list: 1 MR(s)"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["aws", "sts", "get-caller-identity"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "aws sts caller account 123456789012",
        ));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["psql", "-c", "select id, name from users"])
        .assert()
        .success()
        .stdout(predicate::str::contains("psql returned 2 row(s)"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["git", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("git status: 1 modified"));

    suite_cmd()
        .current_dir(root.path())
        .env("PATH", &path_env)
        .args(["gt", "log"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gt log returned 2 stack entries"));
}
