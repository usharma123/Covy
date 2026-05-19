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
fn test_system_more_filter_backed_rtk_wrappers_use_builtin_filters() {
    let root = TempDir::new().unwrap();
    let bin_dir = root.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    write_executable_script(
        &bin_dir.join("ansible-playbook"),
        "#!/bin/sh\nprintf 'PLAY [all] ***\\nok: [web01]\\nTASK [Deploy] ***\\nchanged: [web01]\\nPLAY RECAP ***\\nweb01 : ok=2 changed=1 failed=0\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("fail2ban-client"),
        "#!/bin/sh\nprintf 'Status for the jail: sshd\\n\\n|- Actions\\n   `- Total banned: 42\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("gcloud"),
        "#!/bin/sh\nprintf 'Updated property [core/project].\\n\\nNAME        REGION        STATUS\\nmy-cluster  us-central1   RUNNING\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("hadolint"),
        "#!/bin/sh\nprintf 'Dockerfile:3 DL3008 warning: Pin versions in apt-get install\\n\\nDockerfile:8 DL4006 warning: Set SHELL option -o pipefail before RUN with pipe\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("helm"),
        "#!/bin/sh\nprintf 'W0115 10:30:00 warning message from internal\\nNAME: my-chart\\nSTATUS: deployed\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("iptables"),
        "#!/bin/sh\nprintf 'Chain INPUT (policy ACCEPT)\\nnum  target     prot opt source               destination\\n1    ACCEPT     all  --  0.0.0.0/0            0.0.0.0/0\\nChain DOCKER (1 references)\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("markdownlint"),
        "#!/bin/sh\nprintf 'README.md:1:1 MD041/first-line-heading/first-line-h1 First line in file should be a top level heading\\n\\nREADME.md:15:80 MD013/line-length Line length [Expected: 80; Actual: 95]\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("mix"),
        "#!/bin/sh\ncase \"$1\" in compile) printf 'Compiling 12 files (.ex)\\nGenerated my_app app\\nwarning: variable \"conn\" is unused\\n  lib/router.ex:42\\n' ;; format) printf '' ;; esac; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("ping"),
        "#!/bin/sh\nprintf 'PING example.com (93.184.216.34): 56 data bytes\\n64 bytes from 93.184.216.34: icmp_seq=0 ttl=56 time=14.2 ms\\n\\n--- example.com ping statistics ---\\n4 packets transmitted, 4 packets received, 0.0%% packet loss\\nround-trip min/avg/max/stddev = 13.8/14.0/14.2/0.2 ms\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("pio"),
        "#!/bin/sh\nprintf 'Verbose mode can be enabled via `-v, --verbose` option\\nCompiling .pio/build/esp32dev/src/main.cpp.o\\nsrc/main.cpp:10:3: error: missing symbol\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("pre-commit"),
        "#!/bin/sh\nprintf '[INFO] Installing environment for https://github.com/psf/black.\\nTrim Trailing Whitespace.................................................Passed\\nCheck Yaml...............................................................Failed\\n- hook id: check-yaml\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("ps"),
        "#!/bin/sh\nprintf 'USER   PID %%CPU %%MEM COMMAND\\nroot     1  0.0  0.0 /sbin/launchd\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("quarto"),
        "#!/bin/sh\nprintf 'processing file: index.qmd\\n  Validating schema\\npandoc to html5\\nOutput created: _site/index.html\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("rsync"),
        "#!/bin/sh\nprintf 'sending incremental file list\\n./\\nfile1.txt\\nsent 1,234 bytes  received 42 bytes  2,552.00 bytes/sec\\ntotal size is 98,765  speedup is 77.31\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("shellcheck"),
        "#!/bin/sh\nprintf 'In script.sh line 3:\\nif [[ $1 == \"\" ]]\\n     ^-- SC2236: Use -z instead of ! -n.\\n\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("shopify"),
        "#!/bin/sh\nprintf 'Uploading assets/app.css\\nTheme '\\''Development'\\'' (id: 12345) pushed to store.example.myshopify.com\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("sops"),
        "#!/bin/sh\nprintf 'mac: xyz123\\n\\nversion: 3.8.1\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("systemctl"),
        "#!/bin/sh\nprintf 'nginx.service - A high performance web server\\n\\n     Active: active (running) since Mon 2024-01-15 10:30:00 UTC; 2h ago\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("trunk"),
        "#!/bin/sh\nprintf '   Compiling tokio v1.35.0\\n\\n   Finished release [optimized] target(s) in 45.23s\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("yamllint"),
        "#!/bin/sh\nprintf 'config.yml\\n  3:1     warning  missing document start \"---\"  (document-start)\\n\\n  5:12    error    too many spaces inside braces  (braces)\\n'; exit 0\n",
    );
    write_executable_script(
        &bin_dir.join("liquibase"),
        "#!/bin/sh\nprintf '####################################################\\nStarting Liquibase at 10:12:11 (version 4.29.1)\\nLiquibase Version: 4.29.1\\nLiquibase Open Source 4.29.1 by Liquibase\\nINFO [liquibase.integration] Starting command\\nRunning Changeset: filepath::id::author\\nChangeset filepath::id::author ran successfully\\n'; exit 0\n",
    );
    let path_env = std::env::join_paths(std::iter::once(bin_dir.as_path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();

    for (args, expected) in [
        (vec!["ansible-playbook", "site.yml"], "changed: [web01]"),
        (
            vec!["fail2ban-client", "status", "sshd"],
            "Total banned: 42",
        ),
        (
            vec!["gcloud", "container", "clusters", "list"],
            "my-cluster",
        ),
        (vec!["hadolint", "Dockerfile"], "DL4006 warning"),
        (vec!["helm", "status", "my-chart"], "STATUS: deployed"),
        (vec!["iptables", "-L"], "Chain INPUT (policy ACCEPT)"),
        (vec!["markdownlint", "README.md"], "MD013/line-length"),
        (
            vec!["mix", "compile"],
            "warning: variable \"conn\" is unused",
        ),
        (vec!["mix", "format"], "mix format: ok"),
        (vec!["ping", "example.com"], "4 packets transmitted"),
        (vec!["pio", "run"], "src/main.cpp:10:3: error"),
        (vec!["pre-commit", "run"], "Check Yaml"),
        (vec!["ps"], "/sbin/launchd"),
        (vec!["quarto", "render", "index.qmd"], "ok (output created)"),
        (vec!["rsync", "-a", ".", "remote:"], "ok (synced)"),
        (vec!["shellcheck", "script.sh"], "SC2236"),
        (vec!["shopify", "theme", "push"], "Theme 'Development'"),
        (vec!["sops", "-d", "secrets.yaml"], "version: 3.8.1"),
        (vec!["systemctl", "status", "nginx"], "Active: active"),
        (vec!["trunk", "build"], "Finished release"),
        (vec!["yamllint", "config.yml"], "too many spaces"),
        (vec!["liquibase", "update"], "Running Changeset"),
    ] {
        suite_cmd()
            .current_dir(root.path())
            .env("PATH", &path_env)
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
}
