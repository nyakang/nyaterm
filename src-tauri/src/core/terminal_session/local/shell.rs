fn build_shell_command_from_spec(spec: &ShellCommandSpec) -> (CommandBuilder, String) {
    let mut builder = CommandBuilder::new(&spec.program);
    if !spec.args.is_empty() {
        builder.args(spec.args.iter().map(String::as_str));
    }
    (builder, spec.program.clone())
}

fn default_local_shell_args(program: &str) -> Vec<String> {
    if cfg!(windows) {
        return vec![];
    }

    let shell_name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();

    match shell_name.as_str() {
        "bash" | "zsh" | "fish" => vec!["--login".to_string(), "-i".to_string()],
        _ => vec![],
    }
}

fn default_shell_spec() -> ShellCommandSpec {
    #[cfg(target_os = "windows")]
    {
        ShellCommandSpec {
            program: resolve_program_for_spawn("powershell.exe"),
            args: Vec::new(),
            resolution_source: ShellResolutionSource::Direct,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        ShellCommandSpec {
            args: default_local_shell_args(&program),
            program,
            resolution_source: ShellResolutionSource::Direct,
        }
    }
}

fn resolve_shell_command(shell_path: &str, shell_args: &str) -> Result<ShellCommandSpec, String> {
    let raw_program = shell_path.trim();
    let program = trim_wrapping_quotes(raw_program);
    if program.is_empty() {
        let (_, shell_name) = platform_default_shell();
        let args = parse_shell_args(shell_args)?;
        return Ok(ShellCommandSpec {
            args: if args.is_empty() {
                default_local_shell_args(&shell_name)
            } else {
                args
            },
            program: shell_name,
            resolution_source: ShellResolutionSource::Direct,
        });
    }

    let args = parse_shell_args(shell_args)?;
    #[cfg(target_os = "windows")]
    if is_windows_terminal_alias(program) {
        return Ok(resolve_windows_terminal_default_profile_shell(args.clone())
            .unwrap_or_else(|| fallback_windows_terminal_shell(args)));
    }

    if !args.is_empty() {
        return Ok(ShellCommandSpec {
            program: resolve_program_for_spawn(program),
            args,
            resolution_source: ShellResolutionSource::Direct,
        });
    }

    if should_treat_as_literal_program(raw_program) {
        return Ok(ShellCommandSpec {
            program: resolve_program_for_spawn(program),
            args: default_local_shell_args(program),
            resolution_source: ShellResolutionSource::Direct,
        });
    }

    let mut legacy_parts = parse_shell_args(program)?;
    if legacy_parts.is_empty() {
        return Err("Shell path is required".to_string());
    }
    let legacy_program = legacy_parts.remove(0);
    Ok(ShellCommandSpec {
        program: resolve_program_for_spawn(&legacy_program),
        args: legacy_parts,
        resolution_source: ShellResolutionSource::Direct,
    })
}

