//! Zero-config shell integration for Bash (including 5.3+), Zsh, Fish (including 4.x), and Nushell.
//!
//! Extracts lightweight hook scripts into `<config_dir>/shell-integration/` and
//! configures the spawned shell's arguments/environment so `OSC 7` (working directory)
//! and `OSC 133;A/B/C/D` (`cl=line` / `click_events=1` prompt, command, output, and
//! exit-code markers) are emitted automatically without modifying user dotfiles.

use std::fs;
use std::path::{Path, PathBuf};

const BASH_INTEGRATION_SCRIPT: &str = r#"# Boxxy Terminal Bash Integration (auto-generated)
if [[ "$-" != *i* ]]; then builtin return; fi

if [[ -n "${BOXXY_LOGIN_SHELL:-}" ]]; then
    builtin unset BOXXY_LOGIN_SHELL
    [[ -r /etc/profile ]] && builtin source /etc/profile
    for _boxxy_rc in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [[ -r "$_boxxy_rc" ]]; then
            builtin source "$_boxxy_rc"
            break
        fi
    done
    builtin unset _boxxy_rc
else
    for _boxxy_rc in /etc/bash.bashrc /etc/bash/bashrc /etc/bashrc; do
        if [[ -r "$_boxxy_rc" ]]; then
            builtin source "$_boxxy_rc"
            break
        fi
    done
    builtin unset _boxxy_rc
    [[ -r "$HOME/.bashrc" ]] && builtin source "$HOME/.bashrc"
fi

if [[ -z "${BOXXY_SHELL_INTEGRATION_LOADED:-}" ]]; then
    BOXXY_SHELL_INTEGRATION_LOADED=1
    _boxxy_executing=""
    _boxxy_last_cwd=""

    __boxxy_precmd() {
        builtin local ret="$1"
        if [[ "$_boxxy_executing" != "0" ]]; then
            _BOXXY_SAVE_PS1="$PS1"
            _BOXXY_SAVE_PS2="$PS2"
            PS1='\[\e]133;A;cl=line\a\]'"$PS1"'\[\e]133;B\a\]'
        fi
        if [[ -n "$_boxxy_executing" ]]; then
            builtin printf '\e]133;D;%s\a' "$ret"
        fi
        if [[ "$_boxxy_last_cwd" != "$PWD" ]]; then
            _boxxy_last_cwd="$PWD"
            builtin printf '\e]7;file://%s%s\a' "${HOSTNAME:-localhost}" "$PWD"
        fi
        _boxxy_executing=0
    }

    __boxxy_preexec() {
        PS1="$_BOXXY_SAVE_PS1"
        PS2="$_BOXXY_SAVE_PS2"
        builtin printf '\e]133;C\a'
        _boxxy_executing=1
    }

    if [[ -z "${__boxxy_ps0+x}" ]]; then
        if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 3) )); then
            builtin readonly __boxxy_ps0='${ __boxxy_preexec; }'
        else
            builtin readonly __boxxy_ps0='$(__boxxy_preexec >/dev/tty)'
        fi
    fi

    __boxxy_hook() {
        builtin local ret=$?
        __boxxy_precmd "$ret"
        if [[ "${PS0:-}" != *"$__boxxy_ps0"* ]]; then
            PS0="${PS0:-}${__boxxy_ps0}"
        fi
        return "$ret"
    }

    # Preserve PROMPT_COMMAND whether it is a string or a Bash 5.1+ array
    if [[ ";${PROMPT_COMMAND[*]:-};" != *";__boxxy_hook;"* ]]; then
        if [[ -z "${PROMPT_COMMAND[*]:-}" ]]; then
            if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
                PROMPT_COMMAND=(__boxxy_hook)
            else
                PROMPT_COMMAND="__boxxy_hook"
            fi
        elif [[ $(builtin declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then
            PROMPT_COMMAND+=(__boxxy_hook)
        else
            [[ "${PROMPT_COMMAND}" =~ \;[[:space:]]*$ ]] || PROMPT_COMMAND+=";"
            PROMPT_COMMAND+=" __boxxy_hook"
        fi
    fi
fi
"#;

const ZSH_ENV_SCRIPT: &str = r#"# Boxxy Terminal Zsh .zshenv shim (auto-generated)
if [[ -f "${BOXXY_ORIG_ZDOTDIR:-$HOME}/.zshenv" ]]; then
    source "${BOXXY_ORIG_ZDOTDIR:-$HOME}/.zshenv"
fi
"#;

const ZSH_PROFILE_SCRIPT: &str = r#"# Boxxy Terminal Zsh .zprofile shim (auto-generated)
if [[ -f "${BOXXY_ORIG_ZDOTDIR:-$HOME}/.zprofile" ]]; then
    source "${BOXXY_ORIG_ZDOTDIR:-$HOME}/.zprofile"
fi
"#;

const ZSH_RC_SCRIPT: &str = r#"# Boxxy Terminal Zsh .zshrc shim (auto-generated)
if [[ -n "${BOXXY_ORIG_ZDOTDIR+x}" ]]; then
    export ZDOTDIR="$BOXXY_ORIG_ZDOTDIR"
    unset BOXXY_ORIG_ZDOTDIR
else
    unset ZDOTDIR
fi

if [[ -f "${ZDOTDIR:-$HOME}/.zshrc" ]]; then
    source "${ZDOTDIR:-$HOME}/.zshrc"
fi

if [[ -o interactive ]] && [[ -z "${BOXXY_SHELL_INTEGRATION_LOADED:-}" ]]; then
    typeset -g BOXXY_SHELL_INTEGRATION_LOADED=1
    typeset -gi _boxxy_state=0

    _boxxy_osc7() {
        builtin printf '\e]7;file://%s%s\a' "${HOST:-localhost}" "${PWD}"
    }

    _boxxy_precmd() {
        builtin local -i ret=$?
        if ! builtin zle; then
            if (( _boxxy_state == 1 )); then
                builtin printf '\e]133;D;%d\a' "$ret"
                (( _boxxy_state = 2 ))
            fi
            _boxxy_osc7
            builtin printf '\e]133;A;cl=line\a'
        fi
        if [[ -o prompt_percent ]] && [[ "${PS1:-}" != *$'\e]133;B\a'* ]]; then
            _boxxy_saved_ps1="$PS1"
            PS1+=$'%{\e]133;B\a%}'
            _boxxy_marked_ps1="$PS1"
        fi
    }

    _boxxy_preexec() {
        if [[ -n "${_boxxy_saved_ps1+x}" && "$PS1" == "$_boxxy_marked_ps1" ]]; then
            PS1="$_boxxy_saved_ps1"
        fi
        (( _boxxy_state = 1 ))
        builtin printf '\e]133;C\a'
    }

    autoload -Uz add-zsh-hook
    add-zsh-hook chpwd _boxxy_osc7
    add-zsh-hook precmd _boxxy_precmd
    add-zsh-hook preexec _boxxy_preexec
fi
"#;

const ZSH_LOGIN_SCRIPT: &str = r#"# Boxxy Terminal Zsh .zlogin shim (auto-generated)
if [[ -f "${ZDOTDIR:-$HOME}/.zlogin" ]]; then
    source "${ZDOTDIR:-$HOME}/.zlogin"
fi
"#;

const FISH_INTEGRATION_SCRIPT: &str = r#"# Boxxy Terminal Fish Integration (auto-generated)
function __boxxy_restore_xdg_data_dir
    if not set -q BOXXY_SHELL_INTEGRATION_XDG_DIR
        return
    end
    if set -q XDG_DATA_DIRS
        set --function --path xdg_data_dirs "$XDG_DATA_DIRS"
        if set --function idx (contains --index "$BOXXY_SHELL_INTEGRATION_XDG_DIR" $xdg_data_dirs)
            set --erase --function xdg_data_dirs[$idx]
        end
        if set -q xdg_data_dirs[1]
            set --global --export --unpath XDG_DATA_DIRS "$xdg_data_dirs"
        else
            set --erase --global XDG_DATA_DIRS
        end
    end
    set --erase BOXXY_SHELL_INTEGRATION_XDG_DIR
end

__boxxy_restore_xdg_data_dir
functions -e __boxxy_restore_xdg_data_dir

status --is-interactive; or exit 0

function __boxxy_setup --on-event fish_prompt
    functions -e __boxxy_setup

    function __boxxy_mark_prompt_start --on-event fish_prompt --on-event fish_posterror
        if test "$__boxxy_prompt_state" != prompt-start
            echo -en "\e]133;D\a"
        end
        set --global __boxxy_prompt_state prompt-start
        echo -en "\e]133;A;click_events=1\a"
    end

    function __boxxy_mark_output_start --on-event fish_preexec
        set --global __boxxy_prompt_state pre-exec
        echo -en "\e]133;C\a"
    end

    function __boxxy_mark_output_end --on-event fish_postexec
        set --global __boxxy_prompt_state post-exec
        echo -en "\e]133;D;$status\a"
    end

    function __boxxy_update_cwd_osc --on-variable PWD
        if status --is-command-substitution; or set -q INSIDE_EMACS
            return
        end
        printf \e\]7\;file://%s%s\a $hostname (string escape --style=url $PWD)
    end

    set --global fish_handle_reflow 1
    __boxxy_mark_prompt_start
    __boxxy_update_cwd_osc
end
"#;

const NUSHELL_INTEGRATION_SCRIPT: &str = r#"# Boxxy Terminal Nushell Integration (auto-generated)
if 'BOXXY_SHELL_INTEGRATION_XDG_DIR' in $env {
    if 'XDG_DATA_DIRS' in $env {
        $env.XDG_DATA_DIRS = ($env.XDG_DATA_DIRS | str replace $"($env.BOXXY_SHELL_INTEGRATION_XDG_DIR):" "")
    }
    hide-env BOXXY_SHELL_INTEGRATION_XDG_DIR
}

$env.config = ($env.config? | default {} | upsert shell_integration {
    osc2: true
    osc7: true
    osc8: true
    osc9_9: false
    osc133: true
    osc633: false
    reset_application_mode: true
})
"#;

/// Writes the bundled Bash, Zsh, Fish, and Nushell integration scripts into
/// `<config_dir>/shell-integration/` and returns the path to that directory.
pub fn ensure_integration_scripts() -> Option<PathBuf> {
    let root = boxxy_sys_utils::get_config_dir().join("shell-integration");
    ensure_integration_scripts_in(&root).ok()?;
    Some(root)
}

pub fn ensure_integration_scripts_in(root: &Path) -> std::io::Result<()> {
    let bash_dir = root.join("bash");
    fs::create_dir_all(&bash_dir)?;
    write_if_changed(&bash_dir.join("boxxy-bash.sh"), BASH_INTEGRATION_SCRIPT)?;

    let zsh_dir = root.join("zsh");
    fs::create_dir_all(&zsh_dir)?;
    write_if_changed(&zsh_dir.join(".zshenv"), ZSH_ENV_SCRIPT)?;
    write_if_changed(&zsh_dir.join(".zprofile"), ZSH_PROFILE_SCRIPT)?;
    write_if_changed(&zsh_dir.join(".zshrc"), ZSH_RC_SCRIPT)?;
    write_if_changed(&zsh_dir.join(".zlogin"), ZSH_LOGIN_SCRIPT)?;

    let fish_dir = root.join("fish").join("vendor_conf.d");
    fs::create_dir_all(&fish_dir)?;
    write_if_changed(
        &fish_dir.join("boxxy-shell-integration.fish"),
        FISH_INTEGRATION_SCRIPT,
    )?;

    let nu_dir = root.join("nushell").join("nushell").join("vendor").join("autoload");
    fs::create_dir_all(&nu_dir)?;
    write_if_changed(&nu_dir.join("boxxy.nu"), NUSHELL_INTEGRATION_SCRIPT)?;

    Ok(())
}

fn write_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == content
    {
        return Ok(());
    }
    fs::write(path, content)
}

fn find_env_var(env: &[(String, String)], key: &str) -> Option<String> {
    env.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn prepend_xdg_data_dir(env: &mut Vec<(String, String)>, dir: &Path) {
    let current_xdg = find_env_var(env, "XDG_DATA_DIRS")
        .or_else(|| std::env::var("XDG_DATA_DIRS").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    let dir_str = dir.to_string_lossy().into_owned();
    let new_xdg = if current_xdg.split(':').any(|p| p == dir_str) {
        current_xdg
    } else {
        format!("{dir_str}:{current_xdg}")
    };
    env.push(("BOXXY_SHELL_INTEGRATION_XDG_DIR".to_string(), dir_str));
    env.push(("XDG_DATA_DIRS".to_string(), new_xdg));
    env.push(("BOXXY_SHELL_INTEGRATION".to_string(), "1".to_string()));
}

/// Applies zero-config shell integration arguments and environment variables
/// for interactive `bash`, `zsh`, `fish`, and `nu` (Nushell) spawns.
///
/// Leaves custom commands (`-c`, `--rcfile`, `--norc`, `-f`, etc.) untouched,
/// and respects `BOXXY_SHELL_INTEGRATION=0` as an explicit opt-out.
pub fn apply_shell_integration(
    argv: &mut Vec<String>,
    env: &mut Vec<(String, String)>,
    integration_root: &Path,
) {
    if argv.is_empty() {
        return;
    }

    // Respect explicit opt-out via BOXXY_SHELL_INTEGRATION=0
    if find_env_var(env, "BOXXY_SHELL_INTEGRATION").as_deref() == Some("0") {
        return;
    }

    // Do not inject into non-interactive `-c` commands or custom rc invocations
    let has_custom_flags = argv.iter().skip(1).any(|arg| {
        matches!(
            arg.as_str(),
            "-c" | "--commands"
                | "--rcfile"
                | "--init-file"
                | "--norc"
                | "--posix"
                | "-f"
                | "--no-rcs"
                | "--no-config-file"
        )
    });
    if has_custom_flags {
        return;
    }

    let shell_name = Path::new(&argv[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    match shell_name {
        "bash" => {
            let bash_rc = integration_root.join("bash").join("boxxy-bash.sh");
            if !bash_rc.exists() {
                return;
            }
            let mut is_login = false;
            argv.retain(|arg| {
                if arg == "--login" || arg == "-l" {
                    is_login = true;
                    false
                } else {
                    true
                }
            });
            if is_login {
                env.push(("BOXXY_LOGIN_SHELL".to_string(), "1".to_string()));
            }
            argv.push("--rcfile".to_string());
            argv.push(bash_rc.to_string_lossy().into_owned());
            env.push(("BOXXY_SHELL_INTEGRATION".to_string(), "1".to_string()));
        }
        "zsh" => {
            let zsh_dir = integration_root.join("zsh");
            if !zsh_dir.exists() {
                return;
            }
            if let Some(orig) = find_env_var(env, "ZDOTDIR")
                .or_else(|| std::env::var("ZDOTDIR").ok())
                .filter(|s| !s.is_empty())
            {
                env.push(("BOXXY_ORIG_ZDOTDIR".to_string(), orig));
            }
            env.push((
                "ZDOTDIR".to_string(),
                zsh_dir.to_string_lossy().into_owned(),
            ));
            env.push(("BOXXY_SHELL_INTEGRATION".to_string(), "1".to_string()));
        }
        "fish" => {
            let fish_data_dir = integration_root.join("fish");
            if fish_data_dir.exists() {
                prepend_xdg_data_dir(env, &fish_data_dir);
            }
        }
        "nu" => {
            let nu_data_dir = integration_root.join("nushell");
            if nu_data_dir.exists() {
                prepend_xdg_data_dir(env, &nu_data_dir);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_integration_converts_login_flag_and_injects_rcfile() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_integration_scripts_in(tmp.path()).expect("write scripts");

        let mut argv = vec!["/bin/bash".to_string(), "--login".to_string()];
        let mut env = Vec::new();
        apply_shell_integration(&mut argv, &mut env, tmp.path());

        assert_eq!(argv[0], "/bin/bash");
        assert_eq!(argv[1], "--rcfile");
        assert!(argv[2].ends_with("bash/boxxy-bash.sh"));
        assert!(env.contains(&("BOXXY_LOGIN_SHELL".to_string(), "1".to_string())));
        assert!(env.contains(&("BOXXY_SHELL_INTEGRATION".to_string(), "1".to_string())));
    }

    #[test]
    fn zsh_integration_sets_zdotdir_and_preserves_orig_zdotdir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_integration_scripts_in(tmp.path()).expect("write scripts");

        let custom_zdot = tmp.path().join("custom-zdot").to_string_lossy().into_owned();
        let mut argv = vec!["/usr/bin/zsh".to_string()];
        let mut env = vec![("ZDOTDIR".to_string(), custom_zdot.clone())];
        apply_shell_integration(&mut argv, &mut env, tmp.path());

        assert!(env.contains(&("BOXXY_ORIG_ZDOTDIR".to_string(), custom_zdot)));
        assert_eq!(
            find_env_var(&env, "ZDOTDIR").as_deref(),
            Some(tmp.path().join("zsh").to_string_lossy().as_ref())
        );
    }

    #[test]
    fn fish_and_nushell_integration_prepend_xdg_data_dirs_and_keep_system_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_integration_scripts_in(tmp.path()).expect("write scripts");

        let mut argv = vec!["/usr/bin/fish".to_string()];
        let mut env = vec![("XDG_DATA_DIRS".to_string(), "/usr/share".to_string())];
        apply_shell_integration(&mut argv, &mut env, tmp.path());

        let xdg = find_env_var(&env, "XDG_DATA_DIRS").expect("XDG_DATA_DIRS");
        assert!(xdg.starts_with(tmp.path().join("fish").to_string_lossy().as_ref()));
        assert!(xdg.ends_with(":/usr/share"));

        let mut nu_argv = vec!["/usr/bin/nu".to_string()];
        let mut nu_env = vec![("XDG_DATA_DIRS".to_string(), "/usr/share".to_string())];
        apply_shell_integration(&mut nu_argv, &mut nu_env, tmp.path());
        let nu_xdg = find_env_var(&nu_env, "XDG_DATA_DIRS").expect("XDG_DATA_DIRS");
        assert!(nu_xdg.starts_with(tmp.path().join("nushell").to_string_lossy().as_ref()));
        assert!(
            tmp.path()
                .join("nushell/nushell/vendor/autoload/boxxy.nu")
                .exists()
        );
    }

    #[test]
    fn custom_command_and_opt_out_are_not_modified() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_integration_scripts_in(tmp.path()).expect("write scripts");

        // Non-interactive -c command must not be touched
        let mut argv = vec![
            "/bin/bash".to_string(),
            "-c".to_string(),
            "echo hi".to_string(),
        ];
        let mut env = Vec::new();
        apply_shell_integration(&mut argv, &mut env, tmp.path());
        assert_eq!(argv, vec!["/bin/bash", "-c", "echo hi"]);
        assert!(env.is_empty());

        // Explicit opt-out via BOXXY_SHELL_INTEGRATION=0
        let mut argv2 = vec!["/bin/bash".to_string()];
        let mut env2 = vec![("BOXXY_SHELL_INTEGRATION".to_string(), "0".to_string())];
        apply_shell_integration(&mut argv2, &mut env2, tmp.path());
        assert_eq!(argv2, vec!["/bin/bash"]);
    }
}
