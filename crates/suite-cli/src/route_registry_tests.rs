use super::*;
use std::fs;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

#[test]
fn routes_env_prefixed_reducer_commands() {
    let decision = decide_command_route("FOO=1 cargo test");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(decision.env_assignments.len(), 1);
}

#[test]
fn routes_multiple_env_prefixed_reducer_commands() {
    let decision = decide_command_route("RUST_BACKTRACE=1 CARGO_TERM_COLOR=never cargo check");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        decision.env_assignments,
        vec![
            ("RUST_BACKTRACE".to_string(), "1".to_string()),
            ("CARGO_TERM_COLOR".to_string(), "never".to_string())
        ]
    );
}

#[test]
fn routes_owned_env_values_before_runtime_wrappers_without_reordering() {
    let decision = decide_command_route("EMPTY= EQUALS=left=right command cargo test --workspace");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        decision.env_assignments,
        vec![
            ("EMPTY".to_string(), String::new()),
            ("EQUALS".to_string(), "left=right".to_string()),
        ]
    );
    assert_eq!(decision.wrapper_prefix, vec!["command".to_string()]);
    assert_eq!(decision.argv, ["cargo", "test", "--workspace"]);
}

#[test]
fn disabled_env_prefix_bypasses_rewrite() {
    let decision = decide_command_route("PACKET28_DISABLED=1 git status --short");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("rewrite_disabled"));
}

#[test]
fn rtk_disabled_env_prefix_bypasses_rewrite_for_compatibility() {
    let decision = decide_command_route("FOO=1 RTK_DISABLED=true cargo test");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("rewrite_disabled"));
}

#[test]
fn disabled_env_prefix_false_value_still_routes() {
    let decision = decide_command_route("PACKET28_DISABLED=0 git status --short");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
}

#[test]
fn routes_simple_reads_to_native_tool() {
    let decision = decide_command_route("head -n 20 README.md");
    assert_eq!(decision.kind, RouteKind::NativeTool);
}

#[test]
fn read_rewrites_decline_value_flags_and_multiple_paths() {
    for command in ["head -c 100 README.md", "head README.md CHANGELOG.md"] {
        let decision = decide_command_route(command);
        assert_ne!(decision.kind, RouteKind::NativeTool, "{command}");
    }
}

#[test]
fn native_grep_declines_flags_it_cannot_preserve() {
    for command in [
        "grep -A 5 pattern file",
        "rg -t rust foo",
        "grep -v pattern file",
        "rg -g '*.rs' pat",
    ] {
        let decision = decide_command_route(command);
        assert_ne!(decision.kind, RouteKind::NativeTool, "{command}");
    }
}

#[test]
fn glob_expansion_does_not_rewrite_grep_patterns_or_find_filters() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("Todo"), "").unwrap();
    fs::write(tmp.path().join("src/lib.rs"), "todo\n").unwrap();

    let grep = decide_command_route_with_cwd("grep '[Tt]odo' src", tmp.path());
    assert_eq!(grep.kind, RouteKind::NativeTool);
    assert_eq!(grep.original_argv, None);
    assert!(grep
        .native_tool
        .as_ref()
        .unwrap()
        .argv
        .contains(&"[Tt]odo".to_string()));

    let find = decide_command_route_with_cwd("find . -name '*.rs'", tmp.path());
    assert_eq!(find.kind, RouteKind::RawPassthrough);
    assert_eq!(find.reason.as_deref(), Some("shell_glob"));
}

#[test]
fn routes_cat_line_numbers_like_rtk_and_declines_incompatible_flags() {
    let decision = decide_command_route("cat -n README.md");
    assert_eq!(decision.kind, RouteKind::NativeTool);
    assert_eq!(
        decision.native_tool.as_ref().map(|tool| tool.argv.clone()),
        Some(vec![
            "compact".to_string(),
            "read".to_string(),
            "README.md".to_string()
        ])
    );

    for command in ["cat -A README.md", "cat --show-all README.md"] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::RawPassthrough, "{command}");
        assert_eq!(
            decision.reason.as_deref(),
            Some("unsupported_command"),
            "{command}"
        );
    }
}

#[test]
fn routes_tree_command_to_native_tool() {
    let decision = decide_command_route("tree -L 2 crates");
    assert_eq!(decision.kind, RouteKind::NativeTool);
}

#[test]
fn tolerates_trailing_redirects_for_reducer_routes() {
    let decision = decide_command_route("FOO=1 cargo test -q 2>&1");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(decision.env_assignments.len(), 1);
}

#[test]
fn declines_reducer_commands_with_truncation_pipe() {
    let decision = decide_command_route("git show HEAD | head -n 20");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn routes_supported_compound_commands() {
    let decision = decide_command_route("cargo check && cargo test");
    assert_eq!(decision.kind, RouteKind::CompoundRewrite);
    assert_eq!(decision.reason.as_deref(), Some("compound_rewrite"));
}

#[test]
fn rejects_unsupported_heredoc_commands() {
    let decision = decide_command_route("cat <<EOF\nhello\nEOF");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn declines_pipe_commands_with_supported_left_side() {
    let decision = decide_command_route("git status --short | wc -l");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn builds_compound_rewrite_for_supported_and_mixed_segments() {
    let tmp = tempfile::tempdir().unwrap();
    let decision = decide_command_route_with_cwd_and_root(
        "cargo test && htop || git status --short",
        tmp.path(),
        tmp.path(),
    );
    let rewritten = build_route_rewrite(tmp.path(), "task-compound", None, ".", &decision).unwrap();
    assert!(rewritten.contains("hook reducer-runner"));
    assert!(rewritten.contains(" cargo test"));
    assert!(rewritten.contains("&& htop ||"));
    assert!(rewritten.contains(" git status --short"));
}

#[test]
fn compound_rewrite_preserves_quoted_whitespace() {
    let tmp = tempfile::tempdir().unwrap();
    let decision = decide_command_route_with_cwd_and_root(
        "grep 'a  b' src && git status --short",
        tmp.path(),
        tmp.path(),
    );
    let rewritten =
        build_route_rewrite(tmp.path(), "task-whitespace", None, ".", &decision).unwrap();
    assert!(rewritten.contains("'a  b'"), "{rewritten}");
}

#[test]
fn compound_with_trailing_redirect_passes_through() {
    let decision = decide_command_route("cargo check && cargo test >/dev/null");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn shell_join_preserves_empty_arguments() {
    assert_eq!(
        shell_join(&[
            "git".to_string(),
            "log".to_string(),
            "--author".to_string(),
            String::new()
        ]),
        "git log --author ''"
    );
}

#[test]
fn builds_pipe_rewrite_for_left_side_only_then_resumes_after_chain_operator() {
    let tmp = tempfile::tempdir().unwrap();
    let decision = decide_command_route_with_cwd_and_root(
        "cargo test | grep FAIL && git status --short",
        tmp.path(),
        tmp.path(),
    );
    let rewritten = build_route_rewrite(tmp.path(), "task-pipe", None, ".", &decision).unwrap();
    assert!(rewritten.contains("cargo test | grep FAIL &&"));
    assert!(rewritten.contains(" git status --short"));
    assert_eq!(rewritten.matches("hook reducer-runner").count(), 1);
}

#[test]
fn reducer_rewrite_output_does_not_contain_control_characters() {
    let tmp = tempfile::tempdir().unwrap();
    let decision =
        decide_command_route_with_cwd_and_root("git status --short", tmp.path(), tmp.path());
    let rewritten =
        build_route_rewrite(tmp.path(), "task-safe-output", None, ".", &decision).unwrap();
    assert!(
        rewritten.chars().all(|ch| ch == '\t' || ch >= ' '),
        "{rewritten:?}"
    );
}

#[test]
fn leaves_already_packet28_commands_alone() {
    let decision = decide_command_route("Packet28 run git status --short");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("already_packet28"));
}

#[test]
fn marks_rtk_ignored_commands_as_intentional_passthrough() {
    for command in [
        "cd crates/suite-cli",
        "echo hello",
        "mkdir -p target/tmp",
        "python -c 'print(1)'",
        "node -e 'console.log(1)'",
        "pwd",
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::RawPassthrough, "{command}");
        assert_eq!(
            decision.reason.as_deref(),
            Some("ignored_command"),
            "{command}"
        );
    }
}

#[test]
fn routes_git_global_options_to_reducer() {
    let decision = decide_command_route("git -C crates/suite-cli status --short");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        decision
            .reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.as_str()),
        Some("git_status")
    );
}

#[test]
fn routes_golangci_global_options_before_run_like_rtk() {
    for command in [
        "golangci-lint -v run ./...",
        "golangci-lint --color never run ./...",
        "golangci-lint --color=never run ./...",
        "golangci-lint --config=.golangci.yml run ./...",
        "golangci run ./...",
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("golangci_lint"),
            "{command}"
        );
    }

    let bare = decide_command_route("golangci-lint version");
    assert_eq!(bare.kind, RouteKind::RawPassthrough);
}

#[test]
fn routes_rtk_javascript_package_runner_prefixes() {
    for (command, expected) in [
        ("npm exec tsc --noEmit", "javascript_tsc"),
        ("npm x eslint src", "javascript_eslint"),
        ("npm rum biome check .", "javascript_lint"),
        ("npm run test", "javascript_test"),
        ("npm run-script biome check .", "javascript_lint"),
        ("npm urn jest run", "javascript_test"),
        ("pnpm dlx next build", "javascript_next_build"),
        ("pnpm exec prisma migrate status", "javascript_prisma"),
        ("pnpx vitest run", "javascript_vitest"),
        ("npx jest run", "javascript_test"),
        ("npx playwright test", "javascript_test"),
        ("npx svgo", "javascript_npx"),
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some(expected),
            "{command}"
        );
    }

    let npx = decide_command_route("npx svgo");
    assert!(npx.reducer_spec.as_ref().unwrap().mutation);
}

#[test]
fn routes_rtk_python_package_manager_forms() {
    for (command, expected) in [
        ("python3.11 -m pytest tests", "python_pytest"),
        ("pip3 list", "python_pip_list"),
        ("pip outdated", "python_pip_outdated"),
        ("pip install pytest", "python_pip_install"),
        ("pip show pytest", "python_pip_show"),
        ("uv pip outdated", "python_pip_outdated"),
        ("uv pip install pytest", "python_pip_install"),
        ("uv pip show pytest", "python_pip_show"),
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some(expected),
            "{command}"
        );
    }
}

#[test]
fn routes_rtk_ruff_format_as_mutation() {
    let decision = decide_command_route("ruff format src");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    let spec = decision.reducer_spec.as_ref().unwrap();
    assert_eq!(spec.canonical_kind, "python_ruff_format");
    assert!(spec.mutation);

    let diff = decide_command_route("ruff format --diff src");
    assert_eq!(diff.kind, RouteKind::RawPassthrough);
}

#[test]
fn routes_rtk_docker_and_kubectl_mutation_forms() {
    for (command, expected) in [
        ("docker build .", "docker_build"),
        ("docker compose build", "docker_compose_build"),
        ("docker run alpine echo hi", "docker_run"),
        ("docker exec app ls", "docker_exec"),
        ("kubectl apply -f deploy.yaml", "kubectl_apply"),
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        let spec = decision.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, expected, "{command}");
        assert!(spec.mutation, "{command}");
    }
}

#[test]
fn routes_rtk_cargo_install_as_mutation() {
    let decision = decide_command_route("cargo install ripgrep");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    let spec = decision.reducer_spec.as_ref().unwrap();
    assert_eq!(spec.canonical_kind, "rust_install");
    assert!(spec.mutation);
}

#[test]
fn routes_rtk_cargo_fmt_as_mutation() {
    let decision = decide_command_route("cargo fmt --all");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    let spec = decision.reducer_spec.as_ref().unwrap();
    assert_eq!(spec.canonical_kind, "rust_fmt");
    assert!(spec.mutation);

    let check = decide_command_route("cargo fmt --check");
    assert_eq!(check.kind, RouteKind::ReducerRewrite);
    let spec = check.reducer_spec.as_ref().unwrap();
    assert_eq!(spec.canonical_kind, "rust_fmt");
    assert!(!spec.mutation);
}

#[test]
fn routes_rtk_github_release_and_glab_ci_forms() {
    for (command, expected) in [
        ("gh release list", "gh_release_list"),
        ("glab ci status", "glab_ci_status"),
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        let spec = decision.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, expected, "{command}");
        assert!(!spec.mutation, "{command}");
    }
}

#[test]
fn routes_rtk_psql_connection_forms() {
    for command in ["psql -U postgres -d mydb", "psql postgres://localhost/mydb"] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        let spec = decision.reducer_spec.as_ref().unwrap();
        assert_eq!(spec.canonical_kind, "psql_query", "{command}");
        assert!(!spec.mutation, "{command}");
    }

    let exact_output = decide_command_route("psql -o out.txt -U postgres");
    assert_eq!(exact_output.kind, RouteKind::RawPassthrough);
}

#[test]
fn routes_yadm_like_rtk_git_alias() {
    for (command, expected) in [
        ("yadm status --short", "git_status"),
        ("yadm diff", "git_diff"),
        ("yadm log --oneline", "git_log"),
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some(expected),
            "{command}"
        );
    }
}

#[test]
fn routes_every_upstream_rtk_rule_representative() {
    let tmp = tempfile::tempdir().unwrap();
    for command in [
        "git status --short",
        "yadm status --short",
        "gh pr list",
        "gh release list",
        "glab mr list",
        "glab ci status",
        "cargo build",
        "cargo fmt --all",
        "cargo install ripgrep",
        "pnpm install",
        "npm run test",
        "npm rum lint",
        "npm urn jest run",
        "npx svgo",
        "cat README.md",
        "head -n 5 README.md",
        "tail -n 5 README.md",
        "rg packet README.md",
        "grep packet README.md",
        "ls -la",
        "find . -maxdepth 1",
        "tsc --noEmit",
        "eslint src",
        "biome check .",
        "prettier --check README.md",
        "next build",
        "jest run",
        "vitest run",
        "playwright test",
        "prisma migrate status",
        "docker ps",
        "docker compose ps",
        "docker build .",
        "kubectl get pods",
        "kubectl apply -f deploy.yaml",
        "tree -L 2",
        "diff old.txt new.txt",
        "curl https://example.com",
        "wget https://example.com",
        "python3 -m mypy src",
        "ruff check src",
        "ruff format src",
        "python3.11 -m pytest tests",
        "pip3 list",
        "uv pip install pytest",
        "go test ./...",
        "golangci-lint run ./...",
        "bundle install",
        "bundle exec rails test",
        "bundle exec rspec",
        "bundle exec rubocop",
        "aws sts get-caller-identity",
        "psql postgres://localhost/db",
        "ansible-playbook site.yml",
        "brew install ripgrep",
        "composer install",
        "df -h",
        "dotnet build",
        "du -sh .",
        "fail2ban-client status sshd",
        "gcloud container clusters list",
        "gradle test",
        "./gradlew build",
        "hadolint Dockerfile",
        "helm status app",
        "iptables -L",
        "make test",
        "markdownlint README.md",
        "mix compile",
        "mix format",
        "mvn package",
        "ping example.com",
        "pio run",
        "poetry install",
        "pre-commit run --all-files",
        "ps aux",
        "quarto render index.qmd",
        "rsync -a src remote:",
        "shellcheck script.sh",
        "shopify theme push",
        "sops -d secrets.yaml",
        "swift test",
        "systemctl status nginx",
        "terraform plan",
        "tofu validate",
        "trunk build",
        "uv sync",
        "yamllint config.yml",
        "wc -l README.md",
        "gt log",
        "liquibase update",
    ] {
        let decision = decide_command_route_with_cwd_and_root(command, tmp.path(), tmp.path());
        assert!(
            matches!(
                decision.kind,
                RouteKind::ReducerRewrite | RouteKind::NativeTool | RouteKind::TomlFilterRewrite
            ),
            "{command} routed as {:?} ({:?})",
            decision.kind,
            decision.reason
        );
    }
}

#[test]
fn routes_sudo_and_env_prefixed_commands_like_rtk() {
    let sudo = decide_command_route("sudo git status --short");
    assert_eq!(sudo.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        sudo.reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.as_str()),
        Some("git_status")
    );
    assert_eq!(sudo.argv.first().map(String::as_str), Some("git"));
    assert_eq!(sudo.wrapper_prefix, vec!["sudo".to_string()]);

    let env = decide_command_route("env RUST_BACKTRACE=1 cargo test");
    assert_eq!(env.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        env.reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.as_str()),
        Some("rust_test")
    );
    assert_eq!(
        env.env_assignments,
        vec![("RUST_BACKTRACE".to_string(), "1".to_string())]
    );
}

#[test]
fn routes_rtk_shell_builtin_prefixes_like_rtk() {
    for prefix in ["noglob", "command", "builtin", "exec", "nocorrect"] {
        let command = format!("{prefix} git status");
        let decision = decide_command_route(&command);
        assert_eq!(decision.kind, RouteKind::ReducerRewrite, "{command}");
        assert_eq!(
            decision.wrapper_prefix,
            vec![prefix.to_string()],
            "{command}"
        );
        assert_eq!(
            decision
                .reducer_spec
                .as_ref()
                .map(|spec| spec.canonical_kind.as_str()),
            Some("git_status"),
            "{command}"
        );
    }

    let unknown = decide_command_route("noglob unknown_cmd --flag");
    assert_eq!(unknown.kind, RouteKind::RawPassthrough);
}

#[test]
fn builds_rewrite_with_runtime_wrapper_prefixes() {
    let decision = decide_command_route("sudo noglob git status");
    assert_eq!(
        decision.wrapper_prefix,
        vec!["sudo".to_string(), "noglob".to_string()]
    );
    let rewritten =
        build_route_rewrite(Path::new("/workspace"), "task-prefix", None, ".", &decision).unwrap();
    assert!(rewritten.starts_with("sudo noglob "));
    assert!(rewritten.contains("hook reducer-runner"));
}

#[test]
fn normalizes_absolute_binary_paths_like_rtk() {
    let decision = decide_command_route("/usr/bin/git status --short");
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        decision
            .reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.as_str()),
        Some("git_status")
    );
    assert_eq!(decision.argv.first().map(String::as_str), Some("git"));
}

#[test]
fn repo_config_excludes_commands_from_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("covy.toml"),
        "[packet28.rewrite]\nexclude_commands = [\"git\"]\n",
    )
    .unwrap();
    let decision =
        decide_command_route_with_cwd_and_root("git status --short", tmp.path(), tmp.path());
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
}

#[test]
fn hooks_config_excludes_commands_from_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("covy.toml"),
        "[hooks]\nexclude_commands = [\"cargo\"]\n",
    )
    .unwrap();
    let decision = decide_command_route_with_cwd_and_root("cargo test", tmp.path(), tmp.path());
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
}

#[test]
fn rtk_global_config_excludes_subcommand_patterns_from_rewrite() {
    let _guard = env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config-home");
    fs::create_dir_all(config_home.join("rtk")).unwrap();
    fs::write(
        config_home.join("rtk").join("config.toml"),
        "[hooks]\nexclude_commands = [\"git push\"]\n",
    )
    .unwrap();
    let previous = std::env::var_os("XDG_CONFIG_HOME");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);

    let decision =
        decide_command_route_with_cwd_and_root("git push origin main", tmp.path(), tmp.path());

    if let Some(previous) = previous {
        std::env::set_var("XDG_CONFIG_HOME", previous);
    } else {
        std::env::remove_var("XDG_CONFIG_HOME");
    }
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
}

#[test]
fn rtk_global_config_excludes_regex_patterns_from_rewrite() {
    let _guard = env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config-home");
    fs::create_dir_all(config_home.join("rtk")).unwrap();
    fs::write(
        config_home.join("rtk").join("config.toml"),
        "[hooks]\nexclude_commands = [\"^git (push|tag)\"]\n",
    )
    .unwrap();
    let previous = std::env::var_os("XDG_CONFIG_HOME");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);

    let decision = decide_command_route_with_cwd_and_root("git tag v1.2.3", tmp.path(), tmp.path());

    if let Some(previous) = previous {
        std::env::set_var("XDG_CONFIG_HOME", previous);
    } else {
        std::env::remove_var("XDG_CONFIG_HOME");
    }
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("config_excluded"));
}

#[test]
fn builtin_toml_filters_route_to_packet28_run() {
    let tmp = tempfile::tempdir().unwrap();
    let decision =
        decide_command_route_with_cwd_and_root("brew install rtk", tmp.path(), tmp.path());
    assert_eq!(decision.kind, RouteKind::TomlFilterRewrite);
    assert_eq!(decision.reason.as_deref(), Some("toml_filter"));

    let rewritten = build_route_rewrite(tmp.path(), "task-filter", None, ".", &decision).unwrap();
    assert!(rewritten.contains(" run --root "));
    assert!(rewritten.ends_with(" -- brew install rtk"));

    let swift = decide_command_route_with_cwd_and_root("swift test", tmp.path(), tmp.path());
    assert_eq!(swift.kind, RouteKind::TomlFilterRewrite);
    assert_eq!(swift.reason.as_deref(), Some("toml_filter"));

    let diff =
        decide_command_route_with_cwd_and_root("diff old.txt new.txt", tmp.path(), tmp.path());
    assert_eq!(diff.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        diff.reducer_spec
            .as_ref()
            .map(|spec| spec.canonical_kind.as_str()),
        Some("fs_diff")
    );
}

#[test]
fn repo_config_transparent_prefix_routes_wrapped_command() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("covy.toml"),
        "[packet28.rewrite]\ntransparent_prefixes = [\"poetry run\"]\n",
    )
    .unwrap();
    let decision =
        decide_command_route_with_cwd_and_root("poetry run pytest -q", tmp.path(), tmp.path());
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert_eq!(
        decision.wrapper_prefix,
        vec!["poetry".to_string(), "run".to_string()]
    );
    let rewritten = build_route_rewrite(tmp.path(), "task-prefix", None, ".", &decision).unwrap();
    assert!(rewritten.starts_with("poetry run "));
    assert!(rewritten.contains("hook reducer-runner"));
}

#[test]
fn declines_grep_with_head_pipe_to_preserve_stream_semantics() {
    let decision = decide_command_route("rg task_id crates/suite-cli/src | head -n 5");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn declines_tree_with_head_pipe_to_preserve_stream_semantics() {
    let decision = decide_command_route("tree -L 2 crates | head -n 10");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn declines_cat_with_sed_pipe_to_preserve_stream_semantics() {
    let decision = decide_command_route("cat Cargo.toml | sed -n '1,20p'");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_composition"));
}

#[test]
fn routes_tree_glob_to_native_tool() {
    let decision = decide_command_route("tree crates/*");
    assert_eq!(decision.kind, RouteKind::NativeTool);
}

#[test]
fn routes_grep_glob_path_to_native_tool() {
    let decision = decide_command_route("rg task_id crates/**/*.rs");
    assert_eq!(decision.kind, RouteKind::NativeTool);
}

#[test]
fn declines_extraction_mode_grep_rewrites() {
    for command in [
        "grep -o 'BUG-[0-9]*' bug.md",
        "grep -c BUG bug.md",
        "grep -l BUG bug.md",
        "grep -q BUG bug.md",
        "rg --files-with-matches BUG crates",
        "rg -oc BUG crates",
    ] {
        let decision = decide_command_route(command);
        assert_eq!(decision.kind, RouteKind::RawPassthrough, "{command}");
    }
}

#[test]
fn declines_read_glob_without_shell_expansion() {
    let decision = decide_command_route("cat crates/*/Cargo.toml");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_glob"));
}

#[test]
fn routes_git_diff_glob_with_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    fs::create_dir_all(cwd.join("src")).unwrap();
    fs::write(cwd.join("src/a.rs"), "fn a() {}").unwrap();
    fs::write(cwd.join("src/b.rs"), "fn b() {}").unwrap();
    let decision = decide_command_route_with_cwd("git diff src/*.rs", cwd);
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert!(decision.argv.contains(&"src/a.rs".to_string()));
    assert!(decision.argv.contains(&"src/b.rs".to_string()));
    assert_eq!(
        decision.original_argv,
        Some(vec![
            "git".to_string(),
            "diff".to_string(),
            "src/*.rs".to_string()
        ])
    );
}

#[test]
fn glob_expansion_preserves_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    fs::create_dir_all(cwd.join("src")).unwrap();
    fs::write(cwd.join("src/x.rs"), "fn x() {}").unwrap();
    let decision = decide_command_route_with_cwd("git diff --cached src/*.rs", cwd);
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert!(decision.argv.contains(&"--cached".to_string()));
    assert!(decision.argv.contains(&"src/x.rs".to_string()));
}

#[test]
fn glob_no_matches_keeps_original() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    let decision = decide_command_route_with_cwd("git diff nonexistent/*.rs", cwd);
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_glob"));
}

#[test]
fn glob_without_cwd_still_rejects() {
    let decision = decide_command_route("git diff src/*.rs");
    assert_eq!(decision.kind, RouteKind::RawPassthrough);
    assert_eq!(decision.reason.as_deref(), Some("shell_glob"));
}

#[test]
fn cargo_test_glob_expansion() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    fs::create_dir_all(cwd.join("tests")).unwrap();
    fs::write(cwd.join("tests/test_foo.rs"), "#[test] fn t() {}").unwrap();
    fs::write(
        cwd.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    let decision = decide_command_route_with_cwd("cargo test tests/test_*.rs", cwd);
    assert_eq!(decision.kind, RouteKind::ReducerRewrite);
    assert!(decision.argv.contains(&"tests/test_foo.rs".to_string()));
}
